//! M0-indexed register reads (`v_movrels_b32`): what they read for every M0, run on the host
//! GPU; how many compares their lowering costs; and (ignored, by hand) their GPU time in a
//! loop against the same work written in HLSL as an indexed local array or a switch (`dxc`):
//! `cargo test -p victory-shader --lib movrel_bench -- --ignored --nocapture`.

use std::path::Path;

use super::host::{Dispatch, Host, Root, View};
use super::{HostShader, lower};
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use crate::{io, ir, resource, structure};

const S_ENDPGM: u32 = 0xBF81_0000;
const LIT: u32 = 255;
const M0: u32 = 124;
const SCC_LOOP: u32 = 0x05; // s_cbranch_scc1
/// Rounds of the loop, threads per group, groups per dispatch.
const ROUNDS: u32 = 256;
const THREADS: u32 = 64;
const GROUPS: u32 = 2048;
/// The indexed range the program uses: M0 = round & (SPAN - 1).
const SPAN: u32 = 16;

const fn v(n: u32) -> u32 {
    256 + n
}
const fn k(n: u32) -> u32 {
    128 + n
}
const fn sop1(op: u32, sdst: u32, src0: u32) -> u32 {
    0xBE80_0000 | sdst << 16 | op << 8 | src0
}
const fn sop2(op: u32, sdst: u32, src0: u32, src1: u32) -> u32 {
    0x8000_0000 | op << 23 | sdst << 16 | src1 << 8 | src0
}
const fn sopc(op: u32, src0: u32, src1: u32) -> u32 {
    0xBF00_0000 | op << 16 | src1 << 8 | src0
}
const fn sopp(op: u32, imm: i16) -> u32 {
    0xBF80_0000 | op << 16 | imm as u16 as u32
}
const fn vop1(op: u32, vdst: u32, src0: u32) -> u32 {
    0x7E00_0000 | vdst << 17 | op << 9 | src0
}
const fn vop2(op: u32, vdst: u32, src0: u32, vsrc1: u32) -> u32 {
    op << 25 | vdst << 17 | vsrc1 << 9 | src0
}

/// A console wave32 program of `vgprs` registers: v1..v(vgprs-3) hold `v0 + k`, then `ROUNDS`
/// rounds add `v[1 + (round & (SPAN-1))]` (read with `v_movrels_b32`, M0 uniform) into an
/// accumulator, which each thread stores at dword `v0`.
fn program(vgprs: u32, indexed: bool) -> NativeProgram {
    let (acc, t) = (vgprs - 2, vgprs - 1);
    let mut w = vec![
        0xBE85_0380, // s_mov_b32 s5, 0
        0x3D << 26 | 0x02 << 18 | 8 << 6 | 2,
        125 << 25, // s_load_dwordx4 s[8:11], s[4:5], 0
    ];
    for r in 1..acc {
        if r <= 64 {
            w.push(vop2(0x25, r, k(r), 0)); // v_add_nc_u32 vr, r, v0
        } else {
            w.extend([vop2(0x25, r, LIT, 0), r]);
        }
    }
    w.push(vop1(0x01, acc, k(0))); // v_mov_b32 acc, 0
    w.push(sop1(0x03, 20, k(0))); // s_mov_b32 s20, 0
    let top = w.len();
    w.extend([
        sop2(0x0E, M0, 20, 2), // s_and_b32 m0, s20, s2 (SPAN-1, a root constant: no compiler knows the range)
        if indexed { vop1(0x43, t, v(1)) } else { vop1(0x01, t, v(1)) }, // v_movrels_b32 vt, v1 (or v_mov_b32)
        vop2(0x25, acc, v(t), acc), // v_add_nc_u32 acc, vt, acc
        sop2(0x00, 20, 20, k(1)),  // s_add_u32 s20, s20, 1
        sopc(0x0A, 20, 1),         // s_cmp_lt_u32 s20, s1 (ROUNDS, a root constant)
    ]);
    let back = top as i32 - (w.len() as i32 + 1);
    w.push(sopp(SCC_LOOP, back as i16));
    w.extend([
        vop2(0x1A, 1, k(2), 0), // v_lshlrev_b32 v1, 2, v0
        0xE070_1000,               // buffer_store_dword acc, v1, s[8:11], 0 offen
        0x8002_0000 | acc << 8 | 1,
    ]);
    w.push(S_ENDPGM);
    NativeProgram {
        stage: HwStage::Cs,
        wave64: false,
        vgprs: vgprs as u16,
        layout: vec![
            RootParam {
                root: RootType::Constants,
                table_offset_dwords: 0,
                sgprs: 4,
                base_register: 0,
                user_data: 0,
                slot: 0,
            },
            RootParam {
                root: RootType::Table,
                table_offset_dwords: 0,
                sgprs: 1,
                base_register: 0,
                user_data: 4,
                slot: 1,
            },
        ],
        used: vec![UsedResource {
            kind: ResourceType::Uav,
            root: RootType::Table,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 0,
            dwords: 4,
            user_data: 4,
            slot: 1,
        }],
        io: StageIo::Cs(ComputeIo { rsrc2: 5 << 1, threads: [THREADS, 1, 1] }),
        code: w.iter().flat_map(|w| w.to_le_bytes()).collect(),
    }
}

/// What every thread stores: the sum over the rounds of `v0 + 1 + (round & (SPAN-1))`.
fn want() -> Vec<u32> {
    (0..THREADS)
        .map(|t| (0..ROUNDS).map(|r| t + 1 + (r & (SPAN - 1))).sum())
        .collect()
}

pub(super) fn translated(native: &NativeProgram) -> HostShader {
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let st = structure::structure(&p).expect("structure");
    let res = resource::track(&p, &native.layout, native.stage, Some(&st)).expect("resource::track");
    let io = io::raise(&p, native).expect("io::raise");
    let was = lower::TEST_HOST_WAVE.replace(Some(32));
    let lowered = lower::lower_counted(
        &lower::Parts {
            native,
            p: &p,
            st: &st,
            io: &io,
            res: &res,
            inputs: Vec::new(),
            outputs: Vec::new(),
            prims: Vec::new(),
            mesh_stride: 0,
        },
        false,
    );
    lower::TEST_HOST_WAVE.set(was);
    let lowered = lowered.expect("lowered");
    HostShader {
        stage: HwStage::Cs,
        dxil: lowered.dxil,
        user_data: lowered.user_data,
        samplers: lowered.samplers,
        static_samplers: Vec::new(),
        root_cbvs: lowered.root_cbvs,
        root_srvs: lowered.root_srvs,
        root_uavs: lowered.root_uavs,
        root_indexed_views: res.root_indexed_views,
    }
}

/// The same work in HLSL, with the registers as an indexed local array, compiled by the
/// Windows SDK's `dxc` with the translated shader's bindings; `None` without `dxc`.
fn ideal(like: &HostShader, vgprs: u32, switch: bool) -> Option<HostShader> {
    let dxc = super::validator::Validator::default_path()?.with_file_name("dxc.exe");
    if !dxc.exists() {
        eprintln!("no dxc.exe next to dxil.dll; no HLSL baseline");
        return None;
    }
    let n = vgprs - 2;
    let hlsl = if switch {
        let regs: String = (1..n).map(|k| format!("uint r{k} = t.x + {k};
")).collect();
        let cases: String = (1..n).map(|k| format!("case {k}: x = r{k}; break;
")).collect();
        format!(
            "cbuffer U : register(b0) {{ uint4 c0; uint4 c1; }};
             [numthreads({THREADS}, 1, 1)] void main(uint3 t : SV_GroupThreadID) {{
               RWByteAddressBuffer o = ResourceDescriptorHeap[c1.x / 32];
{regs}               uint acc = 0;
               [loop] for (uint i = 0; i < c0.y; i++) {{ uint x = 0; [branch] switch (1 + (i & c0.z)) {{
{cases}}} acc += x; }}
               o.Store(t.x * 4, acc);
             }}
",
        )
    } else { format!(
        "cbuffer U : register(b0) {{ uint4 c0; uint4 c1; }};\n\
         [numthreads({THREADS}, 1, 1)] void main(uint3 t : SV_GroupThreadID) {{\n\
           RWByteAddressBuffer o = ResourceDescriptorHeap[c1.x / 32];\n\
           uint r[{n}];\n\
           [unroll] for (uint k = 1; k < {n}; k++) r[k] = t.x + k;\n\
           uint acc = 0;\n\
           [loop] for (uint i = 0; i < c0.y; i++) acc += r[1 + (i & c0.z)];\n\
           o.Store(t.x * 4, acc);\n\
         }}\n",
    ) };
    let dir = std::env::temp_dir().join(format!("victory-movrel-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let src = dir.join("ideal.hlsl");
    let out = dir.join("ideal.dxil");
    std::fs::write(&src, hlsl).ok()?;
    let ok = std::process::Command::new(&dxc)
        .args(["-T", "cs_6_6", "-E", "main", "-O3", "-Fo"])
        .arg(&out)
        .arg(&src)
        .status()
        .ok()?
        .success();
    let dxil = std::fs::read(Path::new(&out)).ok().filter(|_| ok)?;
    let _ = std::fs::remove_dir_all(&dir);
    Some(HostShader { dxil, ..like.clone() })
}

fn timed(host: &Host, shader: &HostShader, label: &str) -> Option<f64> {
    let (root, pso) = match host.compute_pipeline(shader) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{label}: the device does not take it: {e}");
            return None;
        }
    };
    let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
    // (the round count is a root constant, s1, so that no compiler unrolls the loop)
    for u in &shader.user_data {
        match (u.root, u.slot, u.dword) {
            (RootType::Constants, 0, 1) => ud[u.index as usize] = ROUNDS,
            (RootType::Constants, 0, 2) => ud[u.index as usize] = SPAN - 1,
            _ => {}
        }
    }
    let d = Dispatch {
        roots: vec![(0, Root::Constants(ud))],
        views: &[View::RawUav(THREADS)],
        samplers: &[],
        groups: [GROUPS, 1, 1],
    };
    let (out, mut ms) = host.dispatch_timed(&root, &pso, &d, 5, 30).unwrap_or_else(|e| panic!("{label}: {e}"));
    assert_eq!(out[0], want(), "{label}: wrong sums");
    ms.sort_by(f64::total_cmp);
    Some(ms[ms.len() / 2])
}

#[test]
#[ignore = "GPU timing; run by hand"]
fn movrel_bench() {
    let Ok(host) = Host::new(false) else {
        eprintln!("no hardware device");
        return;
    };
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for vgprs in [20, 34, 66, 130] {
        let native = program(vgprs, true);
        let ours = translated(&native);
        let t = timed(&host, &ours, "translated");
        let plain = timed_plain(&host, vgprs);
        let i = ideal(&ours, vgprs, false).and_then(|s| timed(&host, &s, "HLSL"));
        let sw = ideal(&ours, vgprs, true).and_then(|s| timed(&host, &s, "HLSL switch"));
        eprintln!("  HLSL registers and a switch on the index: {:.3} ms", sw.unwrap_or(f64::NAN));
        eprintln!(
            "movrels in a loop, {vgprs} VGPRs, {GROUPS}x{THREADS} threads x {ROUNDS} rounds: translated {:.3} ms (same loop with a plain move {:.3} ms), HLSL array {:.3} ms, ratio {:.2}",
            t.unwrap_or(f64::NAN),
            plain,
            i.unwrap_or(f64::NAN),
            t.unwrap_or(f64::NAN) / i.unwrap_or(f64::NAN)
        );
    }
}

/// The translated loop with `v_mov_b32 vt, v1` for the indexed read: its sums are other ones,
/// so it is only timed.
fn timed_plain(host: &Host, vgprs: u32) -> f64 {
    let shader = translated(&program(vgprs, false));
    let (root, pso) = host.compute_pipeline(&shader).expect("plain pipeline");
    let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
    for u in &shader.user_data {
        match (u.root, u.slot, u.dword) {
            (RootType::Constants, 0, 1) => ud[u.index as usize] = ROUNDS,
            (RootType::Constants, 0, 2) => ud[u.index as usize] = SPAN - 1,
            _ => {}
        }
    }
    let d = Dispatch {
        roots: vec![(0, Root::Constants(ud))],
        views: &[View::RawUav(THREADS)],
        samplers: &[],
        groups: [GROUPS, 1, 1],
    };
    let (_, mut ms) = host.dispatch_timed(&root, &pso, &d, 5, 30).expect("plain");
    ms.sort_by(f64::total_cmp);
    ms[ms.len() / 2]
}

/// The register part of [`one_read`]: v1..v(vgprs-2) hold `v0 + k`, the last one holds 7777,
/// then `v_movrels_b32 v(vgprs-1), v1` with M0 = s2. With `sd`, the source side of
/// `v_movrelsd_b32` instead: v1..v(h-1) hold `v0 + k` (h = vgprs / 2) and v(h)..v(vgprs-2) 0,
/// then `v_movrelsd_b32 vh, v1` writes `v[h + M0] = v[1 + M0]` and `v_movrels_b32 v(vgprs-1), vh`
/// reads it back.
fn core(vgprs: u32, sd: bool) -> Vec<u32> {
    let last = vgprs - 1;
    let h = vgprs / 2;
    let mut w = Vec::new();
    for r in 1..last {
        match r {
            _ if sd && r >= h => w.push(vop1(0x01, r, k(0))), // v_mov_b32 vr, 0
            ..=64 => w.push(vop2(0x25, r, k(r), 0)),          // v_add_nc_u32 vr, r, v0
            _ => w.extend([vop2(0x25, r, LIT, 0), r]),
        }
    }
    w.extend([
        vop1(0x01, last, LIT), // v_mov_b32 vlast, 7777
        7777,
        sop1(0x03, M0, 2), // s_mov_b32 m0, s2
    ]);
    if sd {
        w.push(vop1(0x44, h, v(1))); // v_movrelsd_b32 vh, v1
        w.push(vop1(0x43, last, v(h))); // v_movrels_b32 vlast, vh
    } else {
        w.push(vop1(0x43, last, v(1))); // v_movrels_b32 vlast, v1
    }
    w
}

/// A console wave32 program of `vgprs` registers: [`core`], then v(vgprs-1) is stored at dword
/// `v0`. M0 may name `vgprs - 1` registers.
fn one_read(vgprs: u32, sd: bool) -> NativeProgram {
    let last = vgprs - 1;
    let mut native = program(vgprs, true);
    let mut w = vec![
        0xBE85_0380, // s_mov_b32 s5, 0
        0x3D << 26 | 0x02 << 18 | 8 << 6 | 2,
        125 << 25, // s_load_dwordx4 s[8:11], s[4:5], 0
    ];
    w.extend(core(vgprs, sd));
    w.extend([
        vop2(0x1A, 1, k(2), 0), // v_lshlrev_b32 v1, 2, v0
        0xE070_1000,            // buffer_store_dword vlast, v1, s[8:11], 0 offen
        0x8002_0000 | last << 8 | 1,
        S_ENDPGM,
    ]);
    native.code = w.iter().flat_map(|w| w.to_le_bytes()).collect();
    native
}

/// The reference interpreter (`crates/anyxboxx-rdna2-ref`, AGENTS.md rule 4): named by
/// `ANYXBOXX_RDNA2_REF`, or built in the repository's `target`.
fn reference() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("ANYXBOXX_RDNA2_REF") {
        return Some(p.into());
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    ["release", "debug"]
        .iter()
        .map(|p| root.join("target").join(p).join("anyxboxx-rdna2-ref.exe"))
        .find(|p| p.exists())
}

/// What the reference interpreter leaves in v(vgprs-1) after [`core`] with M0 = `m0`, for
/// `THREADS` lanes; `None` where it refuses (a register past the 256 it models).
fn interpreted(exe: &Path, vgprs: u32, sd: bool, m0: u32) -> Option<Vec<u32>> {
    use std::io::Write;
    let words: Vec<String> = core(vgprs, sd).iter().map(|w| format!("{w:08x}")).collect();
    let input = format!("lanes {THREADS}\ns 2 {m0:x}\nprogram {} {S_ENDPGM:08x}\n", words.join(" "));
    let mut child = std::process::Command::new(exe)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("reference interpreter");
    child.stdin.take().expect("stdin").write_all(input.as_bytes()).expect("write");
    let out = String::from_utf8(child.wait_with_output().expect("run").stdout).expect("utf-8");
    if out.starts_with("error") {
        return None;
    }
    let line = format!("v {} ", vgprs - 1);
    Some(match out.lines().find_map(|l| l.strip_prefix(line.as_str())) {
        Some(lanes) => lanes
            .split_whitespace()
            .map(|x| u32::from_str_radix(x, 16).expect("hex"))
            .collect(),
        None => vec![0; THREADS as usize],
    })
}

/// Every M0 reads the register it names, `v[1 + M0]`, while that is one of the program's, and
/// 0 past them (the register numbers wrap no further; the console's result there is not
/// specified, 0 is the translation's): from 0 up to the last register (its 7777) and the first
/// past it, at and past the next power of two of the registers M0 may name, and huge. The
/// register counts are not powers of two: 9 (10 VGPRs), 19 and 129 (130 VGPRs, whose tree is 8
/// levels deep), the cross-check akimfomin suggested. The same reads through the source side
/// of `v_movrelsd_b32` (19 and 129 registers), for every M0 whose write stays in the program's
/// registers. On the hardware adapter and WARP (a missing one is skipped, not both); where the
/// reference interpreter is built, every M0 inside the program's registers is also compared
/// with what it computes.
#[test]
fn an_indexed_read_takes_the_register_m0_names_and_0_past_them() {
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let exe = reference();
    if exe.is_none() {
        eprintln!("no anyxboxx-rdna2-ref built; the comparison with the interpreter is not run");
    }
    let mut ran = 0;
    for warp in [false, true] {
        let Ok(host) = Host::new(warp) else {
            eprintln!("no SM 6.6 device (WARP {warp}); not run there");
            continue;
        };
        ran += 1;
        let mut compared = 0;
        for (vgprs, sd) in [(10u32, false), (20, false), (130, false), (20, true), (130, true)] {
            let shader = translated(&one_read(vgprs, sd));
            let (root, pso) = host.compute_pipeline(&shader).expect("pipeline");
            // M0 names v1..v(vgprs-1): M0 = vgprs - 2 is the last register (7777)
            let (named, pow2) = (vgprs - 1, (vgprs - 1).next_power_of_two());
            let mut m0s: Vec<u32> = (0..=named + 1).collect();
            m0s.extend([pow2 - 1, pow2, pow2 + 1, 2 * pow2, 1000, 0x8000_0000, u32::MAX]);
            let h = vgprs / 2;
            if sd {
                // (the write v[h + M0] stays in the program's registers)
                m0s.retain(|&m| m < vgprs - h);
            }
            for m0 in m0s {
                let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
                for u in &shader.user_data {
                    if let (RootType::Constants, 0, 2) = (u.root, u.slot, u.dword) {
                        ud[u.index as usize] = m0;
                    }
                }
                let d = Dispatch {
                    roots: vec![(0, Root::Constants(ud))],
                    views: &[View::RawUav(THREADS)],
                    samplers: &[],
                    groups: [1, 1, 1],
                };
                let out = host.dispatch(&root, &pso, &d).expect("dispatch");
                let want: Vec<u32> = (0..THREADS)
                    .map(|t| match m0 {
                        m if sd && 1 + m < h => t + 1 + m,
                        _ if sd => 0,
                        m if m < vgprs - 2 => t + 1 + m,
                        m if m == vgprs - 2 => 7777,
                        _ => 0,
                    })
                    .collect();
                let form = if sd { "v_movrelsd_b32" } else { "v_movrels_b32" };
                assert_eq!(out[0], want, "WARP {warp}: {form}, {named} registers, M0 = {m0:#x}");
                if let Some(exe) = exe.as_deref().filter(|_| m0 < vgprs - 1)
                    && let Some(model) = interpreted(exe, vgprs, sd, m0)
                {
                    assert_eq!(out[0], model, "WARP {warp}: {form}, {named} registers, M0 = {m0:#x}: the interpreter's");
                    compared += 1;
                }
            }
        }
        eprintln!("{:?}: every M0 read its register; {compared} compared with the interpreter", host.adapter_name());
    }
    assert!(ran > 0, "no SM 6.6 device");
}

/// An M0-indexed read among the 129 registers v1..v129 costs a few compares on M0's 8 bits (a
/// tree of selects), not one compare per register in a chain each select waits on. Against the
/// same program with a plain move, the indexed read also keeps alive the 126 fills of v2..v127
/// that nothing else reads: 126 + 3 x 8 + 2 at most (the chain added 126 + 129).
#[test]
fn an_indexed_read_compares_m0_bits_not_every_register() {
    let arith = |indexed: bool| {
        let shader = super::census::with(|| translated(&program(130, indexed)));
        let c = super::census::take(&shader.dxil).expect("counted");
        c.counts[super::census::KINDS.iter().position(|k| *k == "arith").expect("kind")]
    };
    let added = arith(true) - arith(false);
    let fills = 126;
    assert!(added <= fills + 3 * 8 + 2, "the indexed read adds {added} arithmetic instructions");
}
