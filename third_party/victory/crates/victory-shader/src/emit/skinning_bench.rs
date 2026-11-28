//! What translated programs cost on the host GPU against the same work in HLSL (`dxc`), all
//! by hand (`-- --ignored --nocapture`), on synthetic programs assembled for gfx1030 with
//! LLVM's `llvm-mc` (the words and their text are in `skin_words.rs`, `probe_words.rs` and
//! `oploop_words.rs`; no title code):
//!
//! - `skinning_bench`: a skinning-shaped program. Per vertex, 4 bone indices and unorm
//!   weights, 3 rows of a current and a previous bone matrix loaded per influence through
//!   descriptors from the table, 24 weighted multiply-adds per influence, then position,
//!   normal and velocity stored; in a loop over the influences and unrolled.
//! - `op_census`: the module instructions one console instruction costs, by kind.
//! - `op_loop_bench` and `op_loop_ideal_bench`: 8 dependent chains of one instruction, 256
//!   rounds, translated and in HLSL.

use std::path::Path;

use super::host::{Dispatch, Host, Root, View};
use super::{HostShader, census, lower};
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use crate::{io, ir, resource, structure};

include!("skin_words.rs");

const THREADS: u32 = 32;
const BONES: u32 = 64;

fn native(code: &[u32]) -> NativeProgram {
    let entry = |kind, at: u32| UsedResource {
        kind,
        root: RootType::Table,
        table_offset_dwords: at,
        lower_bound: 0,
        stride: 0,
        dwords: 4,
        user_data: 4,
        slot: 1,
    };
    NativeProgram {
        stage: HwStage::Cs,
        wave64: false,
        vgprs: 80,
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
        used: vec![
            entry(ResourceType::Srv, 0),
            entry(ResourceType::Srv, 8),
            entry(ResourceType::Srv, 16),
            entry(ResourceType::Uav, 24),
        ],
        io: StageIo::Cs(ComputeIo { rsrc2: 5 << 1 | 1 << 7, threads: [THREADS, 1, 1] }),
        code: code.iter().flat_map(|w| w.to_le_bytes()).collect(),
    }
}

struct Data {
    vertices: Vec<u32>,
    bones: Vec<u32>,
    prev: Vec<u32>,
}

fn data(count: u32) -> Data {
    let mut seed = 0x1234_5678u32;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let unit = |x: u32| ((x % 2000) as f32 / 1000.0 - 1.0).to_bits();
    let mut vertices = Vec::new();
    for _ in 0..count {
        let p: Vec<u32> = (0..6).map(|_| unit(next())).collect();
        let bones = next() & 0x3F3F_3F3F;
        let (a, b, c) = (next() % 128, next() % 64, next() % 32);
        let weights = a | b << 8 | c << 16 | (255 - a - b - c) << 24;
        vertices.extend([p[0], p[1], p[2], p[3], p[4], p[5], bones, weights]);
    }
    let bones = (0..BONES * 12).map(|_| unit(next())).collect();
    let prev = (0..BONES * 12).map(|_| unit(next())).collect();
    Data { vertices, bones, prev }
}

/// What every vertex stores (8 dwords), in f32 on the CPU.
fn want(d: &Data, influences: u32) -> Vec<f32> {
    let f = f32::from_bits;
    let mut out = Vec::new();
    for v in d.vertices.chunks(8) {
        let mut m = [0f32; 24];
        for k in 0..influences {
            let idx = (v[6] >> (8 * k) & 0xFF) as usize;
            let w = (v[7] >> (8 * k) & 0xFF) as f32 * f(0x3b80_8081);
            for j in 0..12 {
                m[j] += w * f(d.bones[idx * 12 + j]);
                m[12 + j] += w * f(d.prev[idx * 12 + j]);
            }
        }
        let (px, py, pz, nx, ny, nz) = (f(v[0]), f(v[1]), f(v[2]), f(v[3]), f(v[4]), f(v[5]));
        let row = |m: &[f32], r: usize| m[4 * r] * px + m[4 * r + 1] * py + m[4 * r + 2] * pz + m[4 * r + 3];
        let pos = [row(&m, 0), row(&m, 1), row(&m, 2)];
        let prev = [row(&m[12..], 0), row(&m[12..], 1)];
        let n = |r: usize| m[4 * r] * nx + m[4 * r + 1] * ny + m[4 * r + 2] * nz;
        out.extend([pos[0], pos[1], pos[2], pos[0] - prev[0], n(0), n(1), n(2), pos[1] - prev[1]]);
    }
    out
}

/// The same work in HLSL with the translated shader's bindings, compiled by the Windows
/// SDK's `dxc`; `None` without it.
fn ideal(like: &HostShader, unrolled: bool) -> Option<HostShader> {
    let dxc = super::validator::Validator::default_path()?.with_file_name("dxc.exe");
    if !dxc.exists() {
        eprintln!("no dxc.exe next to dxil.dll; no HLSL baseline");
        return None;
    }
    let attr = if unrolled { "[unroll]" } else { "[loop]" };
    let count = if unrolled { "4" } else { "c0.x" };
    let hlsl = format!(
        "cbuffer U : register(b0) {{ uint4 c0; uint4 c1; }};
[numthreads({THREADS}, 1, 1)] void main(uint3 g : SV_GroupID, uint3 t : SV_GroupThreadID) {{
  uint base = c1.x / 32;
  ByteAddressBuffer vb = ResourceDescriptorHeap[base];
  ByteAddressBuffer bc = ResourceDescriptorHeap[base + 1];
  ByteAddressBuffer bp = ResourceDescriptorHeap[base + 2];
  RWByteAddressBuffer o = ResourceDescriptorHeap[base + 3];
  uint a = (g.x * {THREADS} + t.x) * 32;
  float4 p = asfloat(vb.Load4(a));
  uint4 q = vb.Load4(a + 16);
  float4 m0 = 0, m1 = 0, m2 = 0, n0 = 0, n1 = 0, n2 = 0;
  {attr} for (uint k = 0; k < {count}; k++) {{
    uint b = ((q.z >> (8 * k)) & 255) * 48;
    float w = float((q.w >> (8 * k)) & 255) * asfloat(0x3b808081u);
    m0 = mad(w, asfloat(bc.Load4(b)), m0);
    m1 = mad(w, asfloat(bc.Load4(b + 16)), m1);
    m2 = mad(w, asfloat(bc.Load4(b + 32)), m2);
    n0 = mad(w, asfloat(bp.Load4(b)), n0);
    n1 = mad(w, asfloat(bp.Load4(b + 16)), n1);
    n2 = mad(w, asfloat(bp.Load4(b + 32)), n2);
  }}
  float3 pos = float3(dot(m0.xyz, p.xyz) + m0.w, dot(m1.xyz, p.xyz) + m1.w, dot(m2.xyz, p.xyz) + m2.w);
  float2 prev = float2(dot(n0.xyz, p.xyz) + n0.w, dot(n1.xyz, p.xyz) + n1.w);
  float3 nv = float3(p.w, asfloat(q.xy));
  float3 nrm = float3(dot(m0.xyz, nv), dot(m1.xyz, nv), dot(m2.xyz, nv));
  o.Store4(a, asuint(float4(pos, pos.x - prev.x)));
  o.Store4(a + 16, asuint(float4(nrm, pos.y - prev.y)));
}}
"
    );
    let dir = std::env::temp_dir().join(format!("victory-skin-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let (src, out) = (dir.join("ideal.hlsl"), dir.join("ideal.dxil"));
    std::fs::write(&src, hlsl).ok()?;
    let ok = std::process::Command::new(&dxc)
        .args(["-T", "cs_6_6", "-E", "main", "-O3", "-Fo"])
        .arg(&out)
        .arg(&src)
        .status()
        .ok()?
        .success();
    let dxil = std::fs::read(Path::new(&out)).ok().filter(|_| ok)?;
    if let Ok(dump) = std::env::var("SKIN_DUMP") {
        let _ = std::fs::write(Path::new(&dump).join(format!("ideal_{unrolled}.dxil")), &dxil);
    }
    let _ = std::fs::remove_dir_all(&dir);
    Some(HostShader { dxil, ..like.clone() })
}

fn timed(host: &Host, shader: &HostShader, d: &Data, groups: u32, label: &str) -> f64 {
    let (root, pso) = host.compute_pipeline(shader).unwrap_or_else(|e| panic!("{label}: {e}"));
    let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
    for u in &shader.user_data {
        if let (RootType::Constants, 0, 0) = (u.root, u.slot, u.dword) {
            ud[u.index as usize] = 4;
        }
    }
    let views = [
        View::DeviceRawSrv(&d.vertices),
        View::DeviceRawSrv(&d.bones),
        View::DeviceRawSrv(&d.prev),
        View::RawUav(groups * THREADS * 8),
    ];
    let dispatch = Dispatch {
        roots: vec![(0, Root::Constants(ud))],
        views: &views,
        samplers: &[],
        groups: [groups, 1, 1],
    };
    let (out, mut ms) =
        host.dispatch_timed(&root, &pso, &dispatch, 5, 40).unwrap_or_else(|e| panic!("{label}: {e}"));
    let want = want(d, 4);
    let worst = out[0]
        .iter()
        .zip(&want)
        .map(|(&g, &w)| (f32::from_bits(g) - w).abs())
        .fold(0f32, f32::max);
    assert!(worst < 1e-3, "{label}: wrong results (worst {worst})");
    ms.sort_by(f64::total_cmp);
    ms[ms.len() / 2]
}

/// `native` translated for a host of 32-lane waves.
fn translated(native: &NativeProgram) -> HostShader {
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let st = structure::structure(&p).expect("structure");
    let res = resource::track(&p, &native.layout, native.stage, Some(&st)).expect("resource::track");
    let io = io::raise(&p, native).expect("io::raise");
    let was = lower::TEST_HOST_WAVE.replace(Some(32));
    let lowered = lower::lower(&lower::Parts {
        native,
        p: &p,
        st: &st,
        io: &io,
        res: &res,
        inputs: Vec::new(),
        outputs: Vec::new(),
        prims: Vec::new(),
        mesh_stride: 0,
    });
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

fn counts(native: &NativeProgram) -> (HostShader, String) {
    let shader = census::with(|| translated(native));
    let c = census::take(&shader.dxil).expect("counted");
    let line = census::KINDS
        .iter()
        .zip(c.counts)
        .filter(|(_, n)| *n != 0)
        .map(|(k, n)| format!("{k}={n}"))
        .collect::<Vec<_>>()
        .join(" ");
    (shader, line)
}

#[test]
#[ignore = "GPU timing; run by hand"]
fn skinning_bench() {
    let Ok(host) = Host::new(false) else {
        eprintln!("no hardware device");
        return;
    };
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let groups: u32 = std::env::var("SKIN_GROUPS").ok().and_then(|s| s.parse().ok()).unwrap_or(8192);
    let d = data(groups * THREADS);
    for (name, code, unrolled) in [("loop", SKIN_LOOP, false), ("unrolled", SKIN_UNROLLED, true)] {
        let (ours, line) = counts(&native(code));
        if let Ok(dir) = std::env::var("SKIN_DUMP") {
            let _ = std::fs::write(Path::new(&dir).join(format!("skin_{name}.dxil")), &ours.dxil);
        }
        let hlsl = ideal(&ours, unrolled);
        let (mut t, mut i) = (Vec::new(), Vec::new());
        for _ in 0..3 {
            t.push(timed(&host, &ours, &d, groups, "translated"));
            if let Some(h) = &hlsl {
                i.push(timed(&host, h, &d, groups, "HLSL"));
            }
        }
        t.sort_by(f64::total_cmp);
        i.sort_by(f64::total_cmp);
        let (t, i) = (t[1], i.get(1).copied().unwrap_or(f64::NAN));
        eprintln!("skinning {name}, {groups}x{THREADS} vertices: translated {t:.3} ms, HLSL {i:.3} ms, ratio {:.2}", t / i);
        eprintln!("  census: {line}");
    }
}

include!("probe_words.rs");

/// What one console instruction costs in the written module: the census of a program that
/// loads 4 dwords, runs it, and stores its result, less that of `v_mov_b32`.
#[test]
#[ignore = "a table, by hand"]
fn op_census() {
    let base = counts(&native(PROBES[0].1)).1;
    eprintln!("base: {base}");
    let parse = |l: &str| -> std::collections::BTreeMap<String, i64> {
        l.split(' ').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_owned(), v.parse().unwrap())).collect()
    };
    let b = parse(&base);
    for (name, words) in PROBES {
        let r = std::panic::catch_unwind(|| counts(&native(words)).1);
        let Ok(line) = r else {
            eprintln!("{name:40} REFUSED");
            continue;
        };
        let c = parse(&line);
        let diff: Vec<String> = c
            .iter()
            .filter(|(k, _)| !["blocks", "threads", "handle", "annotate"].contains(&k.as_str()))
            .map(|(k, v)| (k, v - b.get(k).copied().unwrap_or(0)))
            .filter(|(_, d)| *d != 0)
            .map(|(k, d)| format!("{k}{d:+}"))
            .collect();
        eprintln!("{name:40} {} blocks={}", diff.join(" "), c.get("blocks").copied().unwrap_or(0));
    }
}

include!("oploop_words.rs");

/// GPU time of 8 dependent chains of one console instruction, 256 rounds, 8192 groups of 32
/// threads, translated; and its IR size per instruction.
#[test]
#[ignore = "GPU timing; run by hand"]
fn op_loop_bench() {
    let Ok(host) = Host::new(false) else {
        eprintln!("no hardware device");
        return;
    };
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let groups = 8192u32;
    let input: Vec<u32> = (0..groups * THREADS * 4).map(|i| (1.0f32 + (i % 7) as f32 / 8.0).to_bits()).collect();
    let only = std::env::var("OP_ONLY").ok();
    for (name, words) in OP_LOOPS {
        if only.as_deref().is_some_and(|o| !name.contains(o)) {
            continue;
        }
        let r = std::panic::catch_unwind(|| counts(&native(words)));
        let Ok((shader, line)) = r else {
            eprintln!("{name:24} REFUSED");
            continue;
        };
        let (root, pso) = host.compute_pipeline(&shader).expect("pipeline");
        let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
        for u in &shader.user_data {
            if let (RootType::Constants, 0, 0) = (u.root, u.slot, u.dword) {
                ud[u.index as usize] = 256;
            }
        }
        let views = [
            View::DeviceRawSrv(&input),
            View::DeviceRawSrv(&input[..4]),
            View::DeviceRawSrv(&input[..4]),
            View::RawUav(groups * THREADS * 4),
        ];
        let d = Dispatch { roots: vec![(0, Root::Constants(ud))], views: &views, samplers: &[], groups: [groups, 1, 1] };
        let mut best = Vec::new();
        for _ in 0..3 {
            let (_, mut ms) = host.dispatch_timed(&root, &pso, &d, 3, 15).expect("dispatch");
            ms.sort_by(f64::total_cmp);
            best.push(ms[ms.len() / 2]);
        }
        best.sort_by(f64::total_cmp);
        let total = line.split(' ').find_map(|kv| kv.strip_prefix("total=")).unwrap_or("?");
        eprintln!("{name:24} {:.3} ms  IR total {total}", best[1]);
    }
}

/// The HLSL of one chain step of [`OP_LOOPS`]' instructions (`x` a float, `u` its bits, `a`
/// and `b` the second and third loaded dwords as floats).
const IDEAL_STEPS: &[(&str, &str)] = &[
    ("v_add_f32", "@ = @ + a;"),
    ("v_max_f32", "@ = max(@, a) + b;"),
    ("v_min_f32", "@ = min(@, a) + b;"),
    ("v_med3_f32 (x, 0, 1)", "@ = saturate(@) + b;"),
    ("v_max_f32 (x, 0.5)", "@ = max(@, 0.5) + b;"),
    ("v_max_f32 (x, 0)", "@ = max(@, 0) + b;"),
    ("v_min_f32 (x, 1.0)", "@ = min(@, 1.0) + b;"),
    ("v_med3_f32 (x, -1, 1)", "@ = clamp(@, -1.0, 1.0) + b;"),
    ("v_med3_f32 (x, a, b)", "@ = max(min(@, a), min(max(@, a), b)) + b;"),
    ("v_rndne_f32", "@ = round(@) + b;"),
    ("v_floor_f32", "@ = floor(@) + b;"),
    ("v_sin_f32", "@ = sin(@ * 6.2831853) + b;"),
    ("v_cvt_flr_i32_f32", "@ = float(int(floor(@))) + b;"),
    (
        "v_pk_fma_f16",
        "{ uint u = asuint(@), ua = asuint(a), ub = asuint(b); \
         half2 h = half2(asfloat16(uint16_t(u)), asfloat16(uint16_t(u >> 16))); \
         half2 ha = half2(asfloat16(uint16_t(ua)), asfloat16(uint16_t(ua >> 16))); \
         half2 hb = half2(asfloat16(uint16_t(ub)), asfloat16(uint16_t(ub >> 16))); \
         h = mad(h, ha, hb); @ = asfloat(uint(asuint16(h.x)) | uint(asuint16(h.y)) << 16); }",
    ),
];

#[test]
#[ignore = "GPU timing; run by hand"]
fn op_loop_ideal_bench() {
    let Ok(host) = Host::new(false) else {
        eprintln!("no hardware device");
        return;
    };
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let dxc = super::validator::Validator::default_path().expect("dxil.dll").with_file_name("dxc.exe");
    let groups = 8192u32;
    let input: Vec<u32> = (0..groups * THREADS * 4).map(|i| (1.0f32 + (i % 7) as f32 / 8.0).to_bits()).collect();
    let like = counts(&native(OP_LOOPS[0].1)).0;
    for (name, step) in IDEAL_STEPS {
        let steps: String = (0..8).map(|i| step.replace('@', &format!("x{i}")) + "\n").collect();
        let hlsl = format!(
            "cbuffer U : register(b0) {{ uint4 c0; uint4 c1; }};
[numthreads({THREADS}, 1, 1)] void main(uint3 g : SV_GroupID, uint3 t : SV_GroupThreadID) {{
  uint base = c1.x / 32;
  ByteAddressBuffer vb = ResourceDescriptorHeap[base];
  RWByteAddressBuffer o = ResourceDescriptorHeap[base + 3];
  uint at = (g.x * {THREADS} + t.x) * 16;
  float4 p = asfloat(vb.Load4(at));
  float a = p.y, b = p.z;
  float x0 = p.x, x1 = p.x + 1, x2 = p.x + 2, x3 = p.x + 3, x4 = p.x + 4, x5 = p.x + 5, x6 = p.x + 6, x7 = p.x + 7;
  [loop] for (uint r = 0; r < c0.x; r++) {{
{steps}  }}
  o.Store(at, asuint(x0) ^ asuint(x1) ^ asuint(x2) ^ asuint(x3) ^ asuint(x4) ^ asuint(x5) ^ asuint(x6) ^ asuint(x7));
}}
"
        );
        let dir = std::env::temp_dir().join(format!("victory-op-ideal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp");
        let (src, out) = (dir.join("i.hlsl"), dir.join("i.dxil"));
        std::fs::write(&src, &hlsl).expect("write");
        let ok = std::process::Command::new(&dxc)
            .args(["-T", "cs_6_6", "-E", "main", "-O3", "-enable-16bit-types", "-Fo"])
            .arg(&out)
            .arg(&src)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!("{name:24} dxc failed:\n{hlsl}");
            continue;
        }
        let shader = HostShader { dxil: std::fs::read(&out).expect("dxil"), ..like.clone() };
        let (root, pso) = host.compute_pipeline(&shader).expect("pipeline");
        let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
        for u in &shader.user_data {
            if let (RootType::Constants, 0, 0) = (u.root, u.slot, u.dword) {
                ud[u.index as usize] = 256;
            }
        }
        let views = [
            View::DeviceRawSrv(&input),
            View::DeviceRawSrv(&input[..4]),
            View::DeviceRawSrv(&input[..4]),
            View::RawUav(groups * THREADS * 4),
        ];
        let d = Dispatch { roots: vec![(0, Root::Constants(ud))], views: &views, samplers: &[], groups: [groups, 1, 1] };
        let mut best = Vec::new();
        for _ in 0..3 {
            let (_, mut ms) = host.dispatch_timed(&root, &pso, &d, 3, 15).expect("dispatch");
            ms.sort_by(f64::total_cmp);
            best.push(ms[ms.len() / 2]);
        }
        best.sort_by(f64::total_cmp);
        eprintln!("{name:24} HLSL {:.3} ms", best[1]);
    }
}
