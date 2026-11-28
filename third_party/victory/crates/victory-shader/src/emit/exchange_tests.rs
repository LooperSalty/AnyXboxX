//! A console wave64 compute program run as two host waves of 32 (NVIDIA): what its cross-lane
//! constructs cost, each repeated in a loop, against the same work written in HLSL for waves
//! of 32 (`dxc`). Every run checks the sums against the console's.
//! `cargo test -p victory-shader --lib exchange_bench -- --ignored --nocapture`.

use std::path::Path;

use super::host::{Dispatch, Host, Root, View};
use super::{HostShader, lower};
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use crate::{io, ir, resource, structure};

const S_ENDPGM: u32 = 0xBF81_0000;
const LIT: u32 = 255;
const VCC: u32 = 106;
const EXEC: u32 = 126;
const SCC_LOOP: u32 = 0x05; // s_cbranch_scc1
/// Rounds of the loop, groups per dispatch.
const ROUNDS: u32 = 256;
const GROUPS: u32 = 1024;

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
const fn vopc(op: u32, src0: u32, vsrc1: u32) -> u32 {
    0x7C00_0000 | op << 17 | vsrc1 << 9 | src0
}
const fn vop3(op: u32, dst: u32, src0: u32, src1: u32) -> [u32; 2] {
    [0xD400_0000 | op << 16 | dst, src0 | src1 << 9]
}
/// DS `op vdst, addr, data0` at offset 0.
const fn ds(op: u32, vdst: u32, addr: u32, data0: u32) -> [u32; 2] {
    [0x36 << 26 | op << 18, vdst << 24 | data0 << 8 | addr]
}
const DS_WRITE_B32: u32 = 0x0D;
const DS_READ_B32: u32 = 0x36;

/// The cross-lane constructs, each once a round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Construct {
    /// `v_cmp` into VCC, `s_bcnt1_i32_b64` of it (a lane mask read as a number).
    Ballot,
    /// `v_readlane_b32` at a uniform lane (the round).
    ReadLane,
    /// `v_readfirstlane_b32` of a value that differs by lane.
    FirstLane,
    /// The compiler's wave OR (DPP rows, `v_permlanex16`, `v_readlane` 32 and 0).
    WaveOr,
    /// SCC of `s_and_b64 vcc, exec` read by `s_cselect` ("some lane").
    SomeLane,
    /// Two lane masks read as numbers, from one compare each.
    TwoBallots,
    /// A `ds_write_b32` of the lane's own dword and `reads` reads of other lanes' dwords.
    Lds { reads: u32 },
}

impl Construct {
    pub(super) fn name(self) -> String {
        match self {
            Construct::Ballot => "ballot (s_bcnt1 of a v_cmp)".into(),
            Construct::ReadLane => "v_readlane, uniform lane".into(),
            Construct::FirstLane => "v_readfirstlane".into(),
            Construct::WaveOr => "wave OR idiom (DPP + readlane)".into(),
            Construct::SomeLane => "SCC of a mask (some lane)".into(),
            Construct::TwoBallots => "two ballots in a row".into(),
            Construct::Lds { reads } => format!("LDS: 1 write + {reads} reads"),
        }
    }
}

/// The lane offsets the LDS construct reads (within the console wave).
const LDS_OFFSETS: [u32; 16] = [1, 17, 33, 49, 5, 21, 37, 53, 9, 25, 41, 57, 13, 29, 45, 61];
/// The first VGPR the LDS reads load into (past the addresses, v11..v26).
const LDS_DATA: u32 = 27;

/// A console wave64 program of `threads` threads: `ROUNDS` (s1) rounds of `c`, each adding a
/// result into v10, which each thread stores at dword `v0` of the raw UAV.
pub(super) fn program(c: Construct, threads: u32) -> NativeProgram {
    let mut w = vec![
        0xBE85_0380, // s_mov_b32 s5, 0
        0x3D << 26 | 0x02 << 18 | 8 << 6 | 2,
        125 << 25,              // s_load_dwordx4 s[8:11], s[4:5], 0
        vop2(0x1B, 3, k(63), 0), // v_and_b32 v3, 63, v0 (the console lane)
        vop1(0x01, 10, k(0)),    // v_mov_b32 v10, 0
        sop1(0x03, 20, k(0)),    // s_mov_b32 s20, 0
    ];
    if let Construct::Lds { reads } = c {
        w.extend([
            vop2(0x1A, 8, k(2), 0), // v_lshlrev_b32 v8, 2, v0
            vop2(0x1B, 9, LIT, 0),  // v_and_b32 v9, ~63, v0 (the console wave's first thread)
            0xFFFF_FFC0,
        ]);
        for (n, &o) in LDS_OFFSETS.iter().take(reads as usize).enumerate() {
            let r = 11 + n as u32;
            w.extend([
                vop2(0x25, r, k(o), 0),    // v_add_nc_u32 vr, o, v0
                vop2(0x1B, r, k(63), r),   // v_and_b32 vr, 63, vr
                vop2(0x1C, r, v(9), r),    // v_or_b32 vr, v9, vr
                vop2(0x1A, r, k(2), r),    // v_lshlrev_b32 vr, 2, vr
            ]);
        }
    }
    let top = w.len();
    w.push(vop2(0x25, 4, 20, 0)); // v_add_nc_u32 v4, s20, v0
    match c {
        Construct::Ballot => w.extend([
            vop2(0x25, 5, 20, 3),  // v_add_nc_u32 v5, s20, v3
            vop2(0x1B, 5, k(63), 5), // v_and_b32 v5, 63, v5
            vopc(0xC4, LIT, 5),    // v_cmp_gt_u32 vcc, 20, v5
            20,
            sop1(0x10, 16, VCC),   // s_bcnt1_i32_b64 s16, vcc
            vop2(0x25, 10, 16, 10), // v_add_nc_u32 v10, s16, v10
        ]),
        Construct::TwoBallots => w.extend([
            vop2(0x25, 5, 20, 3),
            vop2(0x1B, 5, k(63), 5),
            vopc(0xC4, LIT, 5), // v_cmp_gt_u32 vcc, 20, v5
            20,
            sop1(0x10, 16, VCC), // s_bcnt1_i32_b64 s16, vcc
            vopc(0xC4, LIT, 5), // v_cmp_gt_u32 vcc, 40, v5
            40,
            sop1(0x10, 17, VCC), // s_bcnt1_i32_b64 s17, vcc
            vop2(0x25, 10, 16, 10),
            vop2(0x25, 10, 17, 10),
        ]),
        Construct::ReadLane => {
            w.extend(vop3(0x360, 14, v(4), 20)); // v_readlane_b32 s14, v4, s20
            w.push(vop2(0x25, 10, 14, 10));
        }
        Construct::FirstLane => w.extend([
            vop1(0x02, 14, v(4)), // v_readfirstlane_b32 s14, v4
            vop2(0x25, 10, 14, 10),
        ]),
        Construct::WaveOr => {
            w.push(vop1(0x01, 7, v(4))); // v_mov_b32 v7, v4
            w.extend_from_slice(&super::wave::WAVE_OR_V7_TO_S7);
            w.push(vop2(0x25, 10, 7, 10)); // v_add_nc_u32 v10, s7, v10
        }
        Construct::SomeLane => w.extend([
            sop2(0x00, 21, 20, 20),   // s_add_u32 s21, s20, s20
            sop2(0x0E, 21, 21, LIT),  // s_and_b32 s21, s21, 127
            127,
            vopc(0xC2, 21, 3),         // v_cmp_eq_u32 vcc, s21, v3
            sop2(0x0F, 18, VCC, EXEC), // s_and_b64 s[18:19], vcc, exec
            0x8506_8081,               // s_cselect_b32 s6, 1, 0
            vop2(0x25, 10, 6, 10),
        ]),
        Construct::Lds { reads } => {
            w.extend(ds(DS_WRITE_B32, 0, 8, 4)); // ds_write_b32 v8, v4
            for n in 0..reads {
                w.extend(ds(DS_READ_B32, LDS_DATA + n, 11 + n, 0)); // ds_read_b32 v(27+n), v(11+n)
            }
            for n in 0..reads {
                w.push(vop2(0x25, 10, v(LDS_DATA + n), 10)); // v_add_nc_u32 v10, v(27+n), v10
            }
        }
    }
    w.extend([
        sop2(0x00, 20, 20, k(1)), // s_add_u32 s20, s20, 1
        sopc(0x0A, 20, 1),        // s_cmp_lt_u32 s20, s1
    ]);
    let back = top as i32 - (w.len() as i32 + 1);
    w.push(sopp(SCC_LOOP, back as i16));
    w.extend([
        vop2(0x1A, 1, k(2), 0), // v_lshlrev_b32 v1, 2, v0
        0xE070_1000,            // buffer_store_dword v10, v1, s[8:11], 0 offen
        0x8002_0000 | 10 << 8 | 1,
        S_ENDPGM,
    ]);
    let lds = if matches!(c, Construct::Lds { .. }) { (threads * 4).div_ceil(512) } else { 0 };
    NativeProgram {
        stage: HwStage::Cs,
        wave64: true,
        vgprs: 48,
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
        io: StageIo::Cs(ComputeIo { rsrc2: 5 << 1 | lds << 15, threads: [threads, 1, 1] }),
        code: w.iter().flat_map(|w| w.to_le_bytes()).collect(),
    }
}

/// What every thread of a group of `threads` stores after `rounds` rounds of `c`, as the
/// console runs it.
pub(super) fn want(c: Construct, threads: u32, rounds: u32) -> Vec<u32> {
    (0..threads)
        .map(|t| {
            let (wave, lane) = (t & !63, t & 63);
            (0..rounds).fold(0u32, |acc, r| {
                let x = match c {
                    Construct::Ballot => (0..64).filter(|l| (l + r) & 63 < 20).count() as u32,
                    Construct::TwoBallots => {
                        (0..64).filter(|l| (l + r) & 63 < 20).count() as u32
                            + (0..64).filter(|l| (l + r) & 63 < 40).count() as u32
                    }
                    Construct::ReadLane => wave + (r & 63) + r,
                    Construct::FirstLane => wave + r,
                    Construct::WaveOr => (0..64).fold(0, |o, l| o | (wave + l + r)),
                    Construct::SomeLane => u32::from((2 * r) & 127 < 64),
                    Construct::Lds { reads } => LDS_OFFSETS
                        .iter()
                        .take(reads as usize)
                        .map(|o| wave + ((lane + o) & 63) + r)
                        .fold(0u32, u32::wrapping_add),
                };
                acc.wrapping_add(x)
            })
        })
        .collect()
}

/// `native` lowered for a host of 32-lane waves (the split form: two host waves a console wave).
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
    assert_eq!(lowered.wave_size, Some(32), "not split into host waves of 32");
    let shader = HostShader {
        stage: HwStage::Cs,
        dxil: lowered.dxil,
        user_data: lowered.user_data,
        samplers: lowered.samplers,
        static_samplers: Vec::new(),
        root_cbvs: lowered.root_cbvs,
        root_srvs: lowered.root_srvs,
        root_uavs: lowered.root_uavs,
        root_indexed_views: res.root_indexed_views,
    };
    if let Ok(mut v) = super::validator::Validator::load(super::validator::Validator::default_path().as_deref()) {
        let mut copy = shader.dxil.clone();
        v.validate(&mut copy).expect("DXIL validation failed");
    }
    shader
}

/// The census of `c`'s translated program, by kind.
pub(super) fn census(c: Construct, threads: u32) -> std::collections::HashMap<&'static str, u32> {
    let shader = super::census::with(|| translated(&program(c, threads)));
    let census = super::census::take(&shader.dxil).expect("counted");
    super::census::KINDS.iter().copied().zip(census.counts).collect()
}

/// The same work in HLSL for waves of 32, compiled by the Windows SDK's `dxc` with the
/// translated shader's bindings: each host wave works out its half's answer and the halves
/// exchange through a double-buffered LDS pair, one group barrier a round (`barriers: 1`);
/// `barriers: 2` is the single pair the translation uses (write, meet, read, meet).
/// `floor`: no exchange at all (each half's own answer, twice): not the console's sums, only
/// what the round costs without meeting. `None` without `dxc`.
fn ideal(like: &HostShader, c: Construct, threads: u32, barriers: u32, floor: bool) -> Option<HostShader> {
    let dxc = super::validator::Validator::default_path()?.with_file_name("dxc.exe");
    if !dxc.exists() {
        eprintln!("no dxc.exe next to dxil.dll; no HLSL baseline");
        return None;
    }
    let pairs = threads.div_ceil(64) * 4;
    // the user-data dwords the translation reads s1 (the round count) and the table from
    let at = |want: (RootType, u8, u32)| {
        like.user_data.iter().find(|u| (u.root, u.slot, u.dword) == want).map(|u| u.index)
    };
    let (rounds, table) = (at((RootType::Constants, 0, 1))?, at((RootType::Table, 1, 0))?);
    let dword = |i: u32| format!("c[{}].{}", i / 4, ["x", "y", "z", "w"][i as usize % 4]);
    let (rounds, table) = (dword(rounds), dword(table));
    // `mine`: this half's part; `both(a, b)`: the round's result from the two halves' parts
    let (mine, both, holder) = match c {
        Construct::Ballot => ("WaveActiveCountBits(((lane + r) & 63) < 20)", "a + b", ""),
        Construct::TwoBallots => (
            "WaveActiveCountBits(((lane + r) & 63) < 20) + WaveActiveCountBits(((lane + r) & 63) < 40)",
            "a + b",
            "",
        ),
        Construct::ReadLane => ("tid + r", "a", "lane == (r & 63)"),
        Construct::FirstLane => ("tid + r", "a", "lane == 0"),
        Construct::WaveOr => ("WaveActiveBitOr(tid + r)", "a | b", ""),
        Construct::SomeLane => ("(uint)WaveActiveAnyTrue(lane == ((2 * r) & 127))", "(a | b) != 0 ? 1u : 0u", ""),
        Construct::Lds { .. } => ("0", "0", ""),
    };
    let body = if let Construct::Lds { reads } = c {
        let sums: String = LDS_OFFSETS
            .iter()
            .take(reads as usize)
            .map(|o| format!("acc += lds[(tid & ~63u) | ((lane + {o}) & 63)];\n"))
            .collect();
        if floor {
            format!("lds[tid] = tid + r;\n{sums}")
        } else {
            format!(
                "lds[tid] = tid + r;\nGroupMemoryBarrierWithGroupSync();\n{sums}GroupMemoryBarrierWithGroupSync();\n"
            )
        }
    } else if floor {
        format!("{{ uint a = {mine}; uint b = a; acc += {both}; }}\n")
    } else {
        let buf = if barriers == 1 { "(r & 1) * 2" } else { "0" };
        let write = if holder.is_empty() {
            format!("uint m = {mine}; if (WaveIsFirstLane()) xs[pair + {buf} + hf] = m;")
        } else {
            format!("uint m = {mine}; if ({holder}) xs[pair + {buf}] = m;")
        };
        let after = if barriers == 2 { "GroupMemoryBarrierWithGroupSync();" } else { "" };
        format!(
            "{{ {write}\nGroupMemoryBarrierWithGroupSync();\nuint a = xs[pair + {buf}], b = xs[pair + {buf} + 1];\n{after}\nacc += {both}; }}\n"
        )
    };
    let hlsl = format!(
        "cbuffer U : register(b0) {{ uint4 c[4]; }};\n\
         groupshared uint xs[{pairs}];\n\
         groupshared uint lds[{threads}];\n\
         [WaveSize(32)] [numthreads({threads}, 1, 1)] void main(uint3 t : SV_GroupThreadID) {{\n\
           RWByteAddressBuffer o = ResourceDescriptorHeap[{table} / 32];\n\
           uint tid = t.x, lane = tid & 63, hf = (tid >> 5) & 1, pair = (tid >> 6) * 4;\n\
           uint acc = 0;\n\
           [loop] for (uint r = 0; r < {rounds}; r++) {{\n{body}}}\n\
           o.Store(tid * 4, acc);\n\
         }}\n",
    );
    let dir = std::env::temp_dir().join(format!("victory-exchange-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let src = dir.join("ideal.hlsl");
    let out = dir.join("ideal.dxil");
    if std::env::var_os("BENCH_SHOW").is_some() {
        eprintln!("{:?}
{hlsl}", like.user_data);
    }
    std::fs::write(&src, &hlsl).ok()?;
    let ok = std::process::Command::new(&dxc)
        .args(["-T", "cs_6_6", "-E", "main", "-O3", "-Fo"])
        .arg(&out)
        .arg(&src)
        .status()
        .ok()?
        .success();
    let dxil = std::fs::read(Path::new(&out)).ok().filter(|_| ok);
    let _ = std::fs::remove_dir_all(&dir);
    if dxil.is_none() {
        eprintln!("dxc refused:\n{hlsl}");
    }
    Some(HostShader { dxil: dxil?, ..like.clone() })
}

/// The median GPU time of `GROUPS` groups of `shader` over 30 dispatches; with `check`, the
/// sums are the console's.
fn timed(host: &Host, shader: &HostShader, threads: u32, check: Option<&[u32]>, label: &str) -> Option<f64> {
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
        if let (RootType::Constants, 0, 1) = (u.root, u.slot, u.dword) {
            ud[u.index as usize] = ROUNDS;
        }
    }
    let d = Dispatch {
        roots: vec![(0, Root::Constants(ud))],
        views: &[View::RawUav(threads)],
        samplers: &[],
        groups: [GROUPS, 1, 1],
    };
    let (out, mut ms) = host.dispatch_timed(&root, &pso, &d, 5, 30).unwrap_or_else(|e| panic!("{label}: {e}"));
    if let Some(want) = check {
        assert_eq!(out[0], want, "{label}: wrong sums");
    }
    ms.sort_by(f64::total_cmp);
    Some(ms[ms.len() / 2])
}

pub(super) const CONSTRUCTS: [Construct; 11] = [
    Construct::Ballot,
    Construct::TwoBallots,
    Construct::ReadLane,
    Construct::FirstLane,
    Construct::WaveOr,
    Construct::SomeLane,
    Construct::Lds { reads: 1 },
    Construct::Lds { reads: 2 },
    Construct::Lds { reads: 4 },
    Construct::Lds { reads: 8 },
    Construct::Lds { reads: 16 },
];

#[test]
#[ignore = "GPU timing; run by hand"]
fn exchange_bench() {
    let Ok(host) = Host::new(false) else {
        eprintln!("no hardware device");
        return;
    };
    eprintln!("adapter: {:?}", host.adapter_name());
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    eprintln!(
        "| construct | threads | barriers (added) | translated | HLSL 2 barriers | HLSL 1 barrier | HLSL no exchange | translated / 1 barrier |"
    );
    for threads in [64, 256] {
        for c in CONSTRUCTS {
            if matches!(c, Construct::Lds { .. }) && threads != 64 {
                continue;
            }
            let native = program(c, threads);
            let counts = census(c, threads);
            let ours = translated(&native);
            let want = want(c, threads, ROUNDS);
            let t = timed(&host, &ours, threads, Some(&want), "translated").unwrap_or(f64::NAN);
            let lds = matches!(c, Construct::Lds { .. });
            let i2 = if lds {
                f64::NAN
            } else {
                ideal(&ours, c, threads, 2, false)
                    .and_then(|s| timed(&host, &s, threads, Some(&want), "HLSL 2 barriers"))
                    .unwrap_or(f64::NAN)
            };
            let i1 = ideal(&ours, c, threads, 1, false)
                .and_then(|s| timed(&host, &s, threads, Some(&want), "HLSL 1 barrier"))
                .unwrap_or(f64::NAN);
            let f = ideal(&ours, c, threads, 0, true)
                .and_then(|s| timed(&host, &s, threads, None, "HLSL floor"))
                .unwrap_or(f64::NAN);
            eprintln!(
                "| {} | {threads} | {} ({}) | {t:.3} ms | {i2:.3} ms | {i1:.3} ms | {f:.3} ms | {:.2} |",
                c.name(),
                counts["barrier"],
                counts["added_barrier"],
                t / i1,
            );
        }
    }
}

/// Every construct gives the console's sums on the host device (one group of 64 and one of
/// 256 threads, 70 rounds: past the 64 lanes of the readlane's lane and the masks' period).
#[test]
fn every_exchange_gives_the_consoles_results() {
    let Ok(host) = Host::new(false) else {
        eprintln!("no hardware device; not run");
        return;
    };
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for threads in [64, 256] {
        for c in CONSTRUCTS {
            if matches!(c, Construct::Lds { .. }) && threads != 64 {
                continue;
            }
            let shader = translated(&program(c, threads));
            let (root, pso) = match host.compute_pipeline(&shader) {
                Ok(x) => x,
                Err(e) => {
                    eprintln!("the device does not take {}: {e}; not run", c.name());
                    return;
                }
            };
            let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
            for u in &shader.user_data {
                if let (RootType::Constants, 0, 1) = (u.root, u.slot, u.dword) {
                    ud[u.index as usize] = 70;
                }
            }
            let d = Dispatch {
                roots: vec![(0, Root::Constants(ud))],
                views: &[View::RawUav(threads)],
                samplers: &[],
                groups: [3, 1, 1],
            };
            let out = host.dispatch(&root, &pso, &d).expect("dispatch");
            assert_eq!(out[0], want(c, threads, 70), "{} in groups of {threads}", c.name());
        }
    }
    eprintln!("adapter {:?}: every construct matched the console", host.adapter_name());
}

/// One LDS access of [`steps_program`], in a round's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    /// `ds_write_b32` of `tid + round + 1000 * n` (n: the step's place) to the thread's dword.
    Write,
    /// `ds_read_b32` of the dword of console lane `lane + offset` (mod 64) of the thread's
    /// console wave, folded in as `acc = (acc << 1) ^ x`.
    Read(u32),
    /// `s_barrier`.
    Barrier,
    /// A ballot (`s_bcnt1` of `v_cmp`: the console lanes below 20 + n, n the step's place),
    /// folded in like a read:
    /// an exchange between the halves, and what puts a group of two console waves in the
    /// split form.
    Ballot,
}

/// A console wave64 program of `threads` threads: `s1` rounds of `steps` (the first a write),
/// each thread storing its fold at dword `v0` of the raw UAV.
pub(super) fn steps_program(steps: &[Step], threads: u32) -> NativeProgram {
    assert_eq!(steps.first(), Some(&Step::Write), "the first step writes");
    let mut w = vec![
        0xBE85_0380, // s_mov_b32 s5, 0
        0x3D << 26 | 0x02 << 18 | 8 << 6 | 2,
        125 << 25,              // s_load_dwordx4 s[8:11], s[4:5], 0
        vop1(0x01, 10, k(0)),   // v_mov_b32 v10, 0
        sop1(0x03, 20, k(0)),   // s_mov_b32 s20, 0
        vop2(0x1A, 8, k(2), 0), // v_lshlrev_b32 v8, 2, v0
        vop2(0x1B, 9, LIT, 0),  // v_and_b32 v9, ~63, v0 (the console wave's first thread)
        0xFFFF_FFC0,
        vop2(0x1B, 5, k(63), 0), // v_and_b32 v5, 63, v0 (the console lane)
    ];
    // each read's address in v11.., in the order of the reads
    let reads: Vec<u32> = steps
        .iter()
        .filter_map(|s| if let Step::Read(o) = s { Some(*o) } else { None })
        .collect();
    assert!(reads.len() <= 16);
    for (n, &o) in reads.iter().enumerate() {
        let r = 11 + n as u32;
        w.extend([
            vop2(0x25, r, LIT, 0), // v_add_nc_u32 vr, o, v0
            o,
            vop2(0x1B, r, k(63), r), // v_and_b32 vr, 63, vr
            vop2(0x1C, r, v(9), r),  // v_or_b32 vr, v9, vr
            vop2(0x1A, r, k(2), r),  // v_lshlrev_b32 vr, 2, vr
        ]);
    }
    let top = w.len();
    w.push(vop2(0x25, 4, 20, 0)); // v_add_nc_u32 v4, s20, v0
    let mut read = 0;
    for (n, s) in steps.iter().enumerate() {
        match *s {
            Step::Write => {
                w.extend([vop2(0x25, 6, LIT, 4), 1000 * n as u32]); // v_add_nc_u32 v6, 1000n, v4
                w.extend(ds(DS_WRITE_B32, 0, 8, 6)); // ds_write_b32 v8, v6
            }
            Step::Read(_) => {
                w.extend(ds(DS_READ_B32, LDS_DATA, 11 + read, 0)); // ds_read_b32 v27, v(11+n)
                w.extend([
                    vop2(0x1A, 10, k(1), 10),        // v_lshlrev_b32 v10, 1, v10
                    vop2(0x1D, 10, v(LDS_DATA), 10), // v_xor_b32 v10, v27, v10
                ]);
                read += 1;
            }
            Step::Barrier => w.push(sopp(0x0A, 0)), // s_barrier
            Step::Ballot => w.extend([
                vopc(0xC4, LIT, 5), // v_cmp_gt_u32 vcc, 20 + n, v5
                20 + n as u32,
                sop1(0x10, 16, VCC),       // s_bcnt1_i32_b64 s16, vcc
                vop2(0x1A, 10, k(1), 10),  // v_lshlrev_b32 v10, 1, v10
                vop2(0x1D, 10, 16, 10),    // v_xor_b32 v10, s16, v10
            ]),
        }
    }
    w.extend([
        sop2(0x00, 20, 20, k(1)), // s_add_u32 s20, s20, 1
        sopc(0x0A, 20, 1),        // s_cmp_lt_u32 s20, s1
    ]);
    let back = top as i32 - (w.len() as i32 + 1);
    w.push(sopp(SCC_LOOP, back as i16));
    w.extend([
        vop2(0x1A, 1, k(2), 0), // v_lshlrev_b32 v1, 2, v0
        0xE070_1000,            // buffer_store_dword v10, v1, s[8:11], 0 offen
        0x8002_0000 | 10 << 8 | 1,
        S_ENDPGM,
    ]);
    // (the layout, bindings and LDS size of the LDS construct's program)
    let mut p = program(Construct::Lds { reads: 1 }, threads);
    p.code = w.iter().flat_map(|w| w.to_le_bytes()).collect();
    p
}

/// What every thread of [`steps_program`] stores after `rounds` rounds, as the console runs
/// it: the wave's lanes go in step, so every lane's write lands before any lane's next access.
pub(super) fn steps_want(steps: &[Step], threads: u32, rounds: u32) -> Vec<u32> {
    let mut lds = vec![0u32; threads as usize];
    let mut acc = vec![0u32; threads as usize];
    for r in 0..rounds {
        for (n, s) in steps.iter().enumerate() {
            match *s {
                Step::Write => {
                    for t in 0..threads {
                        lds[t as usize] = t + r + 1000 * n as u32;
                    }
                }
                Step::Read(o) => {
                    for t in 0..threads {
                        let x = lds[((t & !63) | ((t + o) & 63)) as usize];
                        acc[t as usize] = acc[t as usize] << 1 ^ x;
                    }
                }
                Step::Barrier => {}
                Step::Ballot => acc.iter_mut().for_each(|a| *a = *a << 1 ^ (20 + n as u32)),
            }
        }
    }
    acc
}

/// The census of `steps`' translated program, by kind.
fn steps_census(steps: &[Step], threads: u32) -> std::collections::HashMap<&'static str, u32> {
    let shader = super::census::with(|| translated(&steps_program(steps, threads)));
    let census = super::census::take(&shader.dxil).expect("counted");
    super::census::KINDS.iter().copied().zip(census.counts).collect()
}

use Step::{Ballot as X, Barrier as B, Read as R, Write as W};

/// LDS interleavings a shared read barrier must keep right: a write, a read of the other
/// half, a write over what it read; runs of reads across both halves; back-to-back writes;
/// the title's `s_barrier` or an exchange between reads.
const INTERLEAVINGS: [&[Step]; 9] = [
    &[W, R(32), W, R(32)],
    &[W, R(1), R(33), R(31), R(63), W, R(32), R(0)],
    &[W, R(32), R(17), W, W, R(5)],
    &[W, R(32), B, R(33), R(1)],
    &[W, W, R(40), R(8), R(24), R(56), R(0), R(32)],
    &[W, R(0), W, R(0), W, R(63)],
    &[W, R(32), X, R(33), W, R(1)],
    &[W, R(1), R(33), X, X, R(32), R(31), W, R(0), R(63)],
    &[W, X, X, X, R(32), X, W, X, X],
];

/// The group barriers of the split form: one before each LDS write and one before the first
/// read after a write (or after the title's `s_barrier`), none before a read that follows a
/// read.
#[test]
fn split_lds_reads_share_one_barrier() {
    for reads in [1, 2, 4, 8, 16] {
        let c = census(Construct::Lds { reads }, 64);
        assert_eq!(c["barrier"], 2, "1 write + {reads} reads: {c:?}");
        assert_eq!(c["added_barrier"], 2, "1 write + {reads} reads: {c:?}");
    }
    // (a program that exchanges has one barrier at entry, the thread ticket; each exchange
    // has one, and a run of exchanges in a block one more after the last; a read after an
    // exchange meets again)
    let want: [usize; 9] = [4, 4, 5, 4, 3, 6, 8, 9, 11];
    for (steps, want) in INTERLEAVINGS.iter().zip(want) {
        let c = steps_census(steps, 64);
        // (the title's s_barrier is the program's own, not added)
        let own = steps.iter().filter(|s| **s == B).count();
        assert_eq!(c["barrier"] as usize, want, "{steps:?}: {c:?}");
        assert_eq!(c["added_barrier"] as usize, want - own, "{steps:?}: {c:?}");
    }
}

/// Back-to-back exchanges take the two pairs of exchange dwords in turn and share their
/// meetings: one barrier each, and one after the last of the run (plus the thread ticket's).
#[test]
fn back_to_back_exchanges_share_meetings() {
    for (c, want) in [(Construct::Ballot, 3), (Construct::TwoBallots, 4)] {
        for threads in [64, 256] {
            let counts = census(c, threads);
            assert_eq!(counts["barrier"], want, "{} in groups of {threads}: {counts:?}", c.name());
        }
    }
    // the pairs a run takes in turn (a pair is written again only after a meeting that
    // follows every thread's reads of it); a title read in between does not end the run,
    // the end of the block does
    let pairs = |steps: &[Step]| {
        lower::TEST_EXCHANGE_PAIRS.with(|p| p.borrow_mut().clear());
        translated(&steps_program(steps, 64));
        lower::TEST_EXCHANGE_PAIRS.with(|p| p.take())
    };
    assert_eq!(pairs(&[W, X, X, X, R(32), X]), [false, true, false, true]);
    assert_eq!(pairs(&[W, X]), [false]);
}

/// Rounds and groups of the interleavings' run: enough for the two host waves to drift.
const STEP_ROUNDS: u32 = 256;
const STEP_GROUPS: u32 = 1024;

/// Every interleaving gives the console's folds on the host device, in groups of one console
/// wave, and of two where the program exchanges (without, a group of two console waves is
/// not split).
#[test]
fn split_lds_interleavings_give_the_consoles_results() {
    let Ok(host) = Host::new(false) else {
        eprintln!("no hardware device; not run");
        return;
    };
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for threads in [64, 128] {
        for steps in INTERLEAVINGS {
            if threads > 64 && !steps.contains(&X) {
                continue;
            }
            let shader = translated(&steps_program(steps, threads));
            let (root, pso) = match host.compute_pipeline(&shader) {
                Ok(x) => x,
                Err(e) => {
                    eprintln!("the device does not take {steps:?}: {e}; not run");
                    return;
                }
            };
            let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
            for u in &shader.user_data {
                if let (RootType::Constants, 0, 1) = (u.root, u.slot, u.dword) {
                    ud[u.index as usize] = STEP_ROUNDS;
                }
            }
            let d = Dispatch {
                roots: vec![(0, Root::Constants(ud))],
                views: &[View::RawUav(threads)],
                samplers: &[],
                groups: [STEP_GROUPS, 1, 1],
            };
            let out = host.dispatch(&root, &pso, &d).expect("dispatch");
            assert_eq!(out[0], steps_want(steps, threads, STEP_ROUNDS), "{steps:?} in groups of {threads}");
        }
    }
    eprintln!("adapter {:?}: every interleaving matched the console", host.adapter_name());
}
