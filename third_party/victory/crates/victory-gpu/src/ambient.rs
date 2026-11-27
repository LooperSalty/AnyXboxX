//! A stand-in for the indirect light the title traces rays for.
//!
//! With the ray-tracing structure builder off (`crate::pipeline::builder_runs`)
//! the title's rays find nothing. Its program that resolves the diffuse light
//! of its screen probes (`CS_ScreenProbes_TemporalResolve_*`) then leaves the
//! sky's light outdoors, nothing indoors (walls are black where no lamp or
//! sun reaches) and, in blocks of the picture's tiles, what the probes'
//! memory held. Right after that program the light is raised to an estimate
//! and bounded by a multiple of it.
//!
//! That a stand-in can be put there, and in which of the program's images,
//! is beloooo's find (VIctory-Belobug-AMD, 06031237, 4f45ab88, cd9f78ce,
//! GPL-3.0-or-later), which puts a constant there, shaded in creases from the
//! scene's depth. The estimate here is worked out from the frame instead, so
//! it follows the hour, the room and its lamps by itself: the light the
//! visible surfaces send out (the title keeps it for its own screen traces:
//! `CS_CacheScreenSpaceRadiance`, the frame before) is averaged in 64x36
//! cells, and every point gets the mean of the cells weighted by the share of
//! its surroundings they cover, as a directional value (first-order harmonics
//! at 160x90 points, taken between them by depth).
//!
//! What it assumes: what is not on screen sends out what is (with a slow
//! mean of the last seconds, so that turning into a dark corner is not black
//! at once). No occlusion but the creases'.
//!
//! `VICTORY_AMBIENT`:
//! - unset or `0`/`off`: the title's own output.
//! - a number: that share of the estimate (1: all of it).
//! - `constant:<level>`: the constant, as beloooo's fork has it.
//!
//! `VICTORY_AMBIENT_SHADE=<strength>,<radius in metres>` sets the creases.
//!
//! The air of a room is lit by the same probes: without rays they hold the
//! sky's light indoors too, a brown veil at sunset. `VICTORY_AMBIENT_AIR`
//! bounds what the air scatters by a multiple of the light around.

use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D12::*;
use windows::core::s;

use crate::host::{HostList, host};

/// Programs of the title the stand-in follows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Role {
    /// Resolves the diffuse light.
    Resolve,
    /// Keeps the light visible surfaces send out.
    Radiance,
    /// Adds the lamps' light to what the air scatters (the last program
    /// that writes that volume before it is summed up along the view).
    Air,
}

/// The program's part in this, by its name in the container.
pub(crate) fn role(container: &[u8]) -> Option<Role> {
    let has = |name: &[u8]| container.windows(name.len()).any(|w| w == name);
    if has(b"CS_ScreenProbes_TemporalResolve") {
        Some(Role::Resolve)
    } else if has(b"CS_CacheScreenSpaceRadiance") {
        Some(Role::Radiance)
    } else if has(b"CS_AccumulateLocalLightInscatteringVolume") {
        Some(Role::Air)
    } else {
        None
    }
}

/// Where the resolve writes the diffuse light (R9G9B9E5 as 32-bit integers)
/// and reads the scene's depth, and where the other writes the light of
/// visible surfaces: root parameter and place in its table, in dwords.
pub(crate) const LIGHT: (u8, u32) = (1, 176);
pub(crate) const DEPTH: (u8, u32) = (0, 464);
pub(crate) const RADIANCE: (u8, u32) = (1, 16);
/// Where the light the air scatters is.
pub(crate) const AIR: (u8, u32) = (1, 32);

#[derive(Clone, Copy, PartialEq, Debug)]
enum Set {
    Off,
    Estimate(f32),
    Constant(f32),
}

fn parse(set: &str) -> Set {
    let set = set.trim();
    let number = |t: &str| t.trim().parse::<f32>().ok().filter(|v| v.is_finite() && *v > 0.0);
    match set.strip_prefix("constant:") {
        Some(level) => number(level).map_or(Set::Off, Set::Constant),
        None => number(set).map_or(Set::Off, Set::Estimate),
    }
}

fn set() -> Set {
    static SET: OnceLock<Set> = OnceLock::new();
    *SET.get_or_init(|| match parse(&std::env::var("VICTORY_AMBIENT").unwrap_or_default()) {
        // With the band asked for and no share, the band's level.
        Set::Off => rt_band().map_or(Set::Off, |b| Set::Estimate(b.level)),
        set => set,
    })
}

/// With ray tracing (`VICTORY_AMBIENT_RT=<least>,<most>[,<level>]`; unset:
/// off, the title's own light whenever the builder runs): the diffuse light
/// the title's resolve leaves is kept between `least` and `most` times the
/// estimate (level: the share of the estimate when `VICTORY_AMBIENT` is not
/// set, 0.8). The title's traced light flickers and goes black indoors while
/// the structures its rays walk are partly wrong (reports/RT-FLICKER.md): its
/// detail stays inside the band, its swings and black rooms do not.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Band {
    least: f32,
    most: f32,
    level: f32,
}

pub(crate) fn rt_band() -> Option<Band> {
    static SET: OnceLock<Option<Band>> = OnceLock::new();
    *SET.get_or_init(|| {
        let band = parse_band(&std::env::var("VICTORY_AMBIENT_RT").ok()?)?;
        tracing::info!(least = band.least, most = band.most, level = band.level, "the stand-in keeps the traced indirect light in a band (VICTORY_AMBIENT_RT)");
        Some(band)
    })
}

fn parse_band(set: &str) -> Option<Band> {
    let mut parts = set.split(',').map(|p| p.trim().parse::<f32>().ok().filter(|v| v.is_finite() && *v >= 0.0));
    let least = parts.next().flatten()?;
    let most = parts.next().flatten().unwrap_or(4.0).max(least).max(1e-3);
    let level = parts.next().flatten().filter(|v| *v > 0.0).unwrap_or(0.8);
    Some(Band { least, most, level })
}

/// Whether the stand-in is on: set, and the title's rays find nothing (or
/// the band is asked for with them).
pub(crate) fn on() -> bool {
    set() != Set::Off && (rt_band().is_some() || !crate::pipeline::builder_runs())
}

/// The title's leftover light is kept between these multiples of the
/// estimate: with rays, the band's; without, at least the estimate and at
/// most four times it.
fn bounds() -> (f32, f32) {
    match rt_band() {
        Some(b) if crate::pipeline::builder_runs() => (b.least, b.most),
        _ => (1.0, 4.0),
    }
}

/// The most the air scatters, as a multiple of the light around
/// (`VICTORY_AMBIENT_AIR=<multiple>`; unset or 0: what the title has).
fn air() -> f32 {
    static SET: OnceLock<f32> = OnceLock::new();
    *SET.get_or_init(|| {
        let set = std::env::var("VICTORY_AMBIENT_AIR").unwrap_or_default();
        set.trim().parse().ok().filter(|v: &f32| v.is_finite() && *v > 0.0).unwrap_or(0.0)
    })
}

/// How the slow mean of the light around follows the frames
/// (`VICTORY_AMBIENT_SETTLE=<time constant in ms>[,<jump factor>[,<fresh
/// seconds>]]`; unset: 250,3; 0: 0.04 of the way a frame, as before, which is 25
/// frames: 0.8 s at the console's 30 frames a second, 2-4 s at 7-13). Set,
/// it is the same share of the way a second whatever the frame rate, and a
/// change by more than the jump factor (default 3; 0: none) is taken at
/// once. Fresh seconds (default 0: none): for that long after such a jump
/// the light the title's own resolve left is bounded by the estimate, not
/// four times it, rising back to four times by the end.
#[derive(Clone, Copy)]
struct Settle {
    ms: f32,
    jump: f32,
    fresh: f32,
}

fn settle() -> Option<Settle> {
    static SET: OnceLock<Option<Settle>> = OnceLock::new();
    *SET.get_or_init(|| {
        // Default on (250 ms, jump factor 3: reports/LIGHT-POPIN.md); `0`
        // gives the old 0.04 of the way a frame.
        let set = std::env::var("VICTORY_AMBIENT_SETTLE").unwrap_or_else(|_| "250,3".to_owned());
        let mut parts = set.split(',').map(|p| p.trim().parse::<f32>().ok());
        let ms = parts.next().flatten().filter(|v| v.is_finite() && *v > 0.0)?;
        let jump = parts.next().flatten().filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(3.0);
        let fresh = parts.next().flatten().filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.0);
        tracing::info!(
            ms,
            jump,
            fresh,
            "the stand-in's mean of the light around settles by the clock (VICTORY_AMBIENT_SETTLE)"
        );
        Some(Settle { ms, jump, fresh })
    })
}

/// The share of the way the slow mean goes at this frame's keeping
/// ([`Role::Radiance`]; the other passes do not read it).
fn follow(role: Role) -> f32 {
    match settle() {
        Some(settle) if role == Role::Radiance => 1.0 - (-frame_ms(role) / settle.ms).exp(),
        _ => 0.04,
    }
}

/// Milliseconds since the last keeping ([`Role::Radiance`], once a frame);
/// the first, and after a pause of over a second, as a frame of 30. Other
/// roles: the last answer.
fn frame_ms(role: Role) -> f32 {
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
    static LAST: AtomicU64 = AtomicU64::new(0);
    static DT: AtomicU32 = AtomicU32::new(0x4204_0000); // 33.0
    if role != Role::Radiance {
        return f32::from_bits(DT.load(Relaxed));
    }
    // (Asked twice in a keeping: the words and the share.)
    static ASKED: AtomicU64 = AtomicU64::new(0);
    let now = crate::host::now_ms();
    if ASKED.swap(now, Relaxed) == now {
        return f32::from_bits(DT.load(Relaxed));
    }
    let last = LAST.swap(now, Relaxed);
    let dt = match now.saturating_sub(last) {
        0 => 1.0,
        dt if last == 0 || dt > 1000 => 33.0,
        dt => dt as f32,
    };
    DT.store(dt.to_bits(), Relaxed);
    dt
}

/// Strength and radius of the creases' shade.
fn shade() -> [f32; 2] {
    static SET: OnceLock<[f32; 2]> = OnceLock::new();
    *SET.get_or_init(|| {
        let mut shade = [1.0, 0.6];
        let set = std::env::var("VICTORY_AMBIENT_SHADE").unwrap_or_default();
        for (to, v) in shade.iter_mut().zip(set.split(',')) {
            if let Ok(v) = v.trim().parse() {
                *to = v;
            }
        }
        shade
    })
}

const SOURCE: &str = r#"
#define RS "RootConstants(num32BitConstants=16, b0), DescriptorTable(SRV(t0)), DescriptorTable(UAV(u0)), UAV(u1), UAV(u2), UAV(u3)"
#define CX 64
#define CY 36
#define CELLS (CX * CY)
#define PX 160
#define PY 90
cbuffer Set : register(b0) {
    float level; float constant; float strength; float radius;
    float near; float tan_y; uint width; uint height;
    float follow; float most; float air; uint probe;
    float jump; float fresh; float dt; float least;
};
Texture2D<float> depth : register(t0);
// Cells: the light they send out (then, at CELLS, the slow mean), from
// CELLS + 1 on where they are.
RWStructuredBuffer<float4> cells : register(u1);
// Three a point: red, green and blue, each a mean and a direction.
RWStructuredBuffer<float4> points : register(u2);
// For the light probe (`crate::lightprobe`): sampled pixels, and the sums of
// what the title's resolve left, what was found and what was written.
RWByteAddressBuffer stats : register(u3);
static const float3 LUMA = float3(0.2126, 0.7152, 0.0722);

// The point seen at a texel, in metres from the eye (x right, y up, z away).
float3 seen(int2 p, float2 size) {
    float d = depth.Load(int3(clamp(p, int2(0, 0), int2(size) - 1), 0));
    float z = min(near / max(d, 1e-7), 1000.0);
    float2 ndc = (float2(p) + 0.5) / size * 2.0 - 1.0;
    return float3(ndc.x * tan_y * size.x / size.y * z, -ndc.y * tan_y * z, z);
}

#ifdef KEEP
RWTexture2D<float3> radiance : register(u0);
[RootSignature(RS)]
[numthreads(8, 4, 1)]
void keep(uint3 at : SV_DispatchThreadID) {
    uint w, h;
    radiance.GetDimensions(w, h);
    float3 sum = 0;
    for (uint y = 0; y < 5; y++) for (uint x = 0; x < 5; x++) {
        uint2 p = uint2((float2(at.xy) + (float2(x, y) + 0.5) / 5.0) * float2(w, h) / float2(CX, CY));
        float3 c = max(radiance[p], 0.0);
        c = any(isnan(c)) || any(isinf(c)) ? 0.0 : c;
        // Lamps and glints count, but not for everything.
        sum += c / max(1.0, dot(c, LUMA) / 20.0);
    }
    cells[at.y * CX + at.x] = float4(sum / 25.0, 1.0);
}
groupshared float3 mean_acc[64];
[RootSignature(RS)]
[numthreads(64, 1, 1)]
void mean(uint gi : SV_GroupIndex) {
    float3 sum = 0;
    for (uint i = gi; i < CELLS; i += 64) sum += cells[i].rgb;
    mean_acc[gi] = sum;
    GroupMemoryBarrierWithGroupSync();
    [unroll]
    for (uint s = 32; s > 0; s >>= 1) {
        if (gi < s) mean_acc[gi] += mean_acc[gi + s];
        GroupMemoryBarrierWithGroupSync();
    }
    if (gi != 0) return;
    float4 was = cells[CELLS];
    float3 now = mean_acc[0] / CELLS;
    // (`jump` > 0: a change of more than that factor either way is
    // followed at once, as a new room is.)
    float a = max(dot(now, LUMA), 1e-5), b = max(dot(was.rgb, LUMA), 1e-5);
    bool jumped = jump > 0.0 && max(a / b, b / a) > jump;
    // Its fourth part: seconds since the light around last jumped (from
    // 0.001; 0: nothing kept yet).
    float age = was.a > 0.0 && !jumped ? was.a + dt : 0.001;
    cells[CELLS] = float4(was.a > 0.0 ? lerp(was.rgb, now, jumped ? 1.0 : follow) : now, age);
}
#endif

#ifdef PLACE
[RootSignature(RS)]
[numthreads(8, 4, 1)]
void place(uint3 at : SV_DispatchThreadID) {
    float2 size = float2(width, height);
    float3 p = seen(int2((float2(at.xy) + 0.5) * size / float2(CX, CY)), size);
    cells[CELLS + 1 + at.y * CX + at.x] = float4(p, p.z * p.z);
}
#endif

#ifdef GATHER
[RootSignature(RS)]
[numthreads(8, 2, 1)]
void gather(uint3 at : SV_DispatchThreadID) {
    float2 size = float2(width, height);
    float3 p = seen(int2((float2(at.xy) + 0.5) * size / float2(PX, PY)), size);
    float r2 = 0.25 + 0.0025 * p.z * p.z;
    float whole = 0;
    float3 toward = 0, light = 0, red = 0, green = 0, blue = 0;
    for (uint i = 0; i < CELLS; i++) {
        float4 c = cells[CELLS + 1 + i];
        float3 L = cells[i].rgb;
        float3 d = c.xyz - p;
        float dd = dot(d, d);
        float w = c.w / (dd + r2);
        float3 dir = d / sqrt(dd + 0.25 * r2);
        whole += w;
        toward += w * dir;
        light += w * L;
        red += w * L.r * dir;
        green += w * L.g * dir;
        blue += w * L.b * dir;
    }
    whole = max(whole, 1e-6);
    light /= whole;
    toward /= whole;
    uint at0 = (at.y * PX + at.x) * 3;
    points[at0] = float4(red / whole - light.r * toward, light.r);
    points[at0 + 1] = float4(green / whole - light.g * toward, light.g);
    points[at0 + 2] = float4(blue / whole - light.b * toward, light.b);
}
#endif

#ifdef AIR
RWTexture3D<float3> scattered : register(u0);
// No more than a multiple of the light around: of the room as the last
// seconds showed it, and of the surface seen in this direction (towards a
// window the air outside may be as bright as the sky is).
[RootSignature(RS)]
[numthreads(8, 8, 2)]
void clear_air(uint3 at : SV_DispatchThreadID) {
    uint w, h, d;
    scattered.GetDimensions(w, h, d);
    if (at.x >= w || at.y >= h || at.z >= d) return;
    uint2 q = min(uint2((float2(at.xy) + 0.5) / float2(w, h) * float2(PX, PY)), uint2(PX - 1, PY - 1));
    uint at0 = (q.y * PX + q.x) * 3;
    float3 seen_there = float3(points[at0].w, points[at0 + 1].w, points[at0 + 2].w);
    float far = float(at.z) / float(d);
    float cap = air * lerp(dot(cells[CELLS].rgb, LUMA), dot(seen_there, LUMA), far);
    float3 c = scattered[at];
    float has = dot(c, LUMA);
    if (has > cap) scattered[at] = c * (cap / has);
}
#endif

#ifdef RAISE
RWTexture2D<uint> light : register(u0);
float3 unpack(uint v) {
    return float3(v & 511, (v >> 9) & 511, (v >> 18) & 511) * exp2(float(v >> 27) - 24.0);
}
uint pack(float3 c) {
    c = clamp(c, 0.0, 65408.0);
    float top = max(c.r, max(c.g, c.b));
    int e = clamp(int(floor(log2(max(top, 1e-9)))) + 16, 0, 31);
    float scale = exp2(float(24 - e));
    if (top * scale >= 511.5) { e = min(e + 1, 31); scale = exp2(float(24 - e)); }
    uint3 m = min(uint3(c * scale + 0.5), 511u);
    return m.r | (m.g << 9) | (m.b << 18) | (uint(e) << 27);
}
// 32x32 groups (1024 threads, D3D12 max) keep each 32x32 pixel tile resident on
// one SM so the 12-tap +/-128 px spiral depth taps share L1 texture cache and
// saturate SM occupancy without hitting per-SM CTA limits.
[RootSignature(RS)]
[numthreads(32, 32, 1)]
void raise(uint3 at : SV_DispatchThreadID) {
    float2 size = float2(width, height);
    if (at.x >= width || at.y >= height) return;
    int2 p = int2(at.xy);
    float3 c = seen(p, size);
    float3 l = seen(p + int2(-1, 0), size), r = seen(p + int2(1, 0), size);
    float3 u = seen(p + int2(0, -1), size), d = seen(p + int2(0, 1), size);
    float3 dx = abs(l.z - c.z) < abs(r.z - c.z) ? c - l : r - c;
    float3 dy = abs(u.z - c.z) < abs(d.z - c.z) ? c - u : d - c;
    float3 n = normalize(cross(dy, dx) + float3(0, 0, -1e-9));
    if (dot(n, c) > 0.0) n = -n;
    // How open it is (as in beloooo's cd9f78ce).
    float open = 1.0;
    float px = min(radius * size.y / (2.0 * tan_y * c.z), 128.0);
    float noise = frac(52.9829189 * frac(dot(float2(p), float2(0.06711056, 0.00583715))));
    if (strength > 0.0 && px >= 1.5 && c.z < 400.0) {
        float sum = 0.0;
        const int N = 12;
        for (int i = 0; i < N; i++) {
            float a = i * 2.399963 + noise * 6.2831853;
            float2 o = float2(cos(a), sin(a)) * (sqrt((i + 0.5) / N) * px);
            float3 v = seen(p + int2(round(o)), size) - c;
            float vv = dot(v, v);
            float over = dot(v, n) - 0.004 * c.z - 0.005;
            sum += max(over, 0.0) / sqrt(vv + 1e-6) * saturate(1.0 - vv / (radius * radius));
        }
        open = saturate(1.0 - strength * 2.0 * sum / N);
    }
    float3 found;
    if (constant > 0.0) {
        found = float3(1.0, 0.98, 0.94) * constant * open;
        light[p] = pack(found);
        return;
    }
    // Between the four points around, those at its own depth first; which
    // four is dithered a little.
    float2 g = (float2(p) + 0.5) / size * float2(PX, PY) - 0.5 + (noise - 0.5) * 0.5;
    int2 g0 = int2(floor(g));
    float2 f = g - float2(g0);
    float4 sr = 0, sg = 0, sb = 0;
    float sum = 0;
    for (int k = 0; k < 4; k++) {
        int2 q = clamp(g0 + int2(k & 1, k >> 1), int2(0, 0), int2(PX, PY) - 1);
        float z = seen(int2((float2(q) + 0.5) * size / float2(PX, PY)), size).z;
        float2 b = float2(k & 1 ? f.x : 1 - f.x, k >> 1 ? f.y : 1 - f.y);
        float w = (b.x * b.y + 1e-3) / (0.02 + abs(z - c.z) / c.z);
        uint at0 = (q.y * PX + q.x) * 3;
        sr += w * points[at0];
        sg += w * points[at0 + 1];
        sb += w * points[at0 + 2];
        sum += w;
    }
    float3 all = float3(sr.w, sg.w, sb.w) / sum;
    found = all + 1.2 * float3(dot(sr.xyz, n), dot(sg.xyz, n), dot(sb.xyz, n)) / sum;
    found = clamp(found, 0.2 * all, 2.5 * all);
    found = lerp(found, cells[CELLS].rgb, 0.2) * level * open;
    float3 was = unpack(light[p]);
    float left = dot(was, LUMA);
    // Magenta (green far below red and blue) is what the probes' memory held, not light.
    if (was.g < 0.5 * min(was.r, was.b) && max(was.r, was.b) > 0.02) was = 0.0;
    // What the program left counts as far as it can be true: the sky's light
    // outdoors, not the blocks of other memory.
    // For `fresh` seconds after the light around jumped (a new room) the
    // title's leftover counts for no more than the estimate, then up to
    // `most` times it again: its history holds the place before.
    float bound = fresh > 0.0 ? lerp(1.0, most, saturate(cells[CELLS].a / fresh)) : most;
    float top = max(dot(found, LUMA) * bound, 1e-6);
    float has = dot(was, LUMA);
    was *= min(1.0, top / max(has, 1e-6));
    float3 written = max(was, found * least);
    light[p] = pack(written);
    if (probe != 0 && (at.x & 15) == 0 && (at.y & 15) == 0) {
        uint ignored;
        stats.InterlockedAdd(0, 1, ignored);
        stats.InterlockedAdd(4, uint(min(left, 1000.0) * 256.0), ignored);
        stats.InterlockedAdd(8, uint(min(dot(found, LUMA), 1000.0) * 256.0), ignored);
        stats.InterlockedAdd(12, uint(min(dot(written, LUMA), 1000.0) * 256.0), ignored);
    }
}
#endif
"#;

/// Thread-group width/height of [`SOURCE`]'s `raise` pass.
const RAISE_GROUP: u32 = 32;

/// Bytes of the cells' buffer and of the points'.
const CELLS: u64 = 64 * 36;
const CELL_BYTES: u64 = (2 * CELLS + 1) * 16;
const POINT_BYTES: u64 = 160 * 90 * 48;
/// Where the slow mean is among the cells (float4s).
pub(crate) const CELLS_AT: u32 = CELLS as u32;

struct Pass {
    root: ID3D12RootSignature,
    keep: ID3D12PipelineState,
    mean: ID3D12PipelineState,
    place: ID3D12PipelineState,
    gather: ID3D12PipelineState,
    raise: ID3D12PipelineState,
    air: ID3D12PipelineState,
    cells: ID3D12Resource,
    points: ID3D12Resource,
    stats: ID3D12Resource,
}
// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for Pass {}
// SAFETY: as above.
unsafe impl Sync for Pass {}

fn make(device: &ID3D12Device, buffer: impl Fn(u64) -> Option<ID3D12Resource>) -> windows::core::Result<Pass> {
    let mut root: Option<ID3D12RootSignature> = None;
    let mut pipeline = |part: &str, entry| -> windows::core::Result<ID3D12PipelineState> {
        let code = crate::host::compile(&format!("#define {part}\n{SOURCE}"), entry, s!("cs_5_1"))?;
        // SAFETY: the root signature is inside the shader; plain creations.
        unsafe {
            if root.is_none() {
                root = Some(device.CreateRootSignature(0, &code)?);
            }
            let desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
                pRootSignature: std::mem::transmute_copy(&root),
                CS: D3D12_SHADER_BYTECODE { pShaderBytecode: code.as_ptr().cast(), BytecodeLength: code.len() },
                ..Default::default()
            };
            device.CreateComputePipelineState(&desc)
        }
    };
    let keep = pipeline("KEEP", s!("keep"))?;
    let mean = pipeline("KEEP", s!("mean"))?;
    let place = pipeline("PLACE", s!("place"))?;
    let gather = pipeline("GATHER", s!("gather"))?;
    let raise = pipeline("RAISE", s!("raise"))?;
    let air = pipeline("AIR", s!("clear_air"))?;
    let made = |bytes| buffer(bytes).ok_or_else(windows::core::Error::empty);
    Ok(Pass {
        root: root.ok_or_else(windows::core::Error::empty)?,
        keep,
        mean,
        place,
        gather,
        raise,
        air,
        cells: made(CELL_BYTES)?,
        points: made(POINT_BYTES)?,
        stats: made(256)?,
    })
}

fn pass() -> Option<&'static Pass> {
    static PASS: OnceLock<Option<Pass>> = OnceLock::new();
    PASS.get_or_init(|| {
        let host = host()?;
        make(&host.device, |bytes| host.uav_buffer(bytes))
            .inspect_err(|e| tracing::warn!(error = %e, "no stand-in for the indirect light"))
            .ok()
    })
    .as_ref()
}

/// The probe's sums and the cells (with the slow mean), once the passes are
/// made and the stand-in is on.
pub(crate) fn probe_buffers() -> Option<(ID3D12Resource, ID3D12Resource)> {
    if !on() {
        return None;
    }
    pass().map(|p| (p.stats.clone(), p.cells.clone()))
}

fn uav_barrier(list: &ID3D12GraphicsCommandList) {
    let barrier = D3D12_RESOURCE_BARRIER { Type: D3D12_RESOURCE_BARRIER_TYPE_UAV, ..Default::default() };
    // SAFETY: an open list; the barrier names no resource.
    unsafe { list.ResourceBarrier(&[barrier]) };
}

/// Whether the light of a frame has been kept: the estimate has something
/// to begin with.
static KEPT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What the passes read and write, as views in the heap the list has set.
pub(crate) struct Views {
    /// The image the title's program just wrote.
    pub(crate) image: D3D12_GPU_DESCRIPTOR_HANDLE,
    /// The scene's depth, for [`Role::Resolve`].
    pub(crate) depth: D3D12_GPU_DESCRIPTOR_HANDLE,
    /// The size of the image, and how many slices it has.
    pub(crate) size: [u32; 2],
    pub(crate) slices: u32,
}

/// After the title's program of that role, in `list` with the program's view
/// heap set. Replaces the list's compute root signature and pipeline; false
/// if it recorded nothing.
pub(crate) fn after(list: &HostList, role: Role, views: &Views) -> bool {
    use std::sync::atomic::Ordering;
    let Some(pass) = pass() else {
        return false;
    };
    let (level, constant) = match set() {
        Set::Off => return false,
        Set::Estimate(level) => (level, 0.0f32),
        Set::Constant(level) => (0.0, level),
    };
    if role != Role::Radiance && constant == 0.0 && !KEPT.load(Ordering::Relaxed) {
        return false;
    }
    if role == Role::Air && (constant > 0.0 || air() == 0.0) {
        return false;
    }
    static SAID: Mutex<u32> = Mutex::new(0);
    {
        let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
        if *said < 4 {
            *said += 1;
            tracing::info!(?role, size = ?views.size, level, constant, "a stand-in for the indirect light");
        }
    }
    list.keep(&pass.cells);
    list.keep(&pass.points);
    list.keep(&pass.stats);
    let [strength, radius] = shade();
    let floats = [level, constant, strength, radius, 0.1, 0.45];
    let mut words = [0u32; 16];
    for (to, v) in words.iter_mut().zip(floats) {
        *to = v.to_bits();
    }
    words[6] = views.size[0];
    words[7] = views.size[1];
    words[8] = follow(role).to_bits();
    let (least, most) = bounds();
    words[9] = most.to_bits();
    words[15] = least.to_bits();
    words[10] = air().to_bits();
    words[11] = u32::from(crate::lightprobe::on());
    words[12] = settle().map_or(0.0, |s| s.jump).to_bits();
    words[13] = settle().map_or(0.0, |s| s.fresh).to_bits();
    words[14] = (frame_ms(role) / 1000.0).to_bits();
    let l = &list.list;
    // SAFETY: an open list with the heap of the views set; the title's
    // program has just used both views, so their textures are in the states
    // these views need; buffers the list keeps.
    unsafe {
        uav_barrier(l);
        l.SetComputeRootSignature(&pass.root);
        l.SetComputeRoot32BitConstants(0, 16, words.as_ptr().cast(), 0);
        l.SetComputeRootDescriptorTable(1, views.depth);
        l.SetComputeRootDescriptorTable(2, views.image);
        l.SetComputeRootUnorderedAccessView(3, pass.cells.GetGPUVirtualAddress());
        l.SetComputeRootUnorderedAccessView(4, pass.points.GetGPUVirtualAddress());
        l.SetComputeRootUnorderedAccessView(5, pass.stats.GetGPUVirtualAddress());
        let run = |pso: &ID3D12PipelineState, x, y, z| {
            l.SetPipelineState(pso);
            l.Dispatch(x, y, z);
            uav_barrier(l);
        };
        match role {
            Role::Radiance => {
                run(&pass.keep, 8, 9, 1);
                run(&pass.mean, 1, 1, 1);
                KEPT.store(true, Ordering::Relaxed);
            }
            Role::Air => run(&pass.air, views.size[0].div_ceil(8), views.size[1].div_ceil(8), views.slices.div_ceil(2)),
            Role::Resolve => {
                if constant == 0.0 {
                    run(&pass.place, 8, 9, 1);
                    run(&pass.gather, 20, 45, 1);
                }
                run(
                    &pass.raise,
                    views.size[0].div_ceil(RAISE_GROUP),
                    views.size[1].div_ceil(RAISE_GROUP),
                    1,
                );
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings() {
        assert_eq!(parse(""), Set::Off);
        assert_eq!(parse("off"), Set::Off);
        assert_eq!(parse("0"), Set::Off);
        assert_eq!(parse("0.8"), Set::Estimate(0.8));
        assert_eq!(parse("constant:0.3"), Set::Constant(0.3));
        assert_eq!(parse("constant:nan"), Set::Off);
    }

    #[test]
    fn the_band_with_rays() {
        assert_eq!(parse_band("0.5,1.5"), Some(Band { least: 0.5, most: 1.5, level: 0.8 }));
        assert_eq!(parse_band("1,1,0.6"), Some(Band { least: 1.0, most: 1.0, level: 0.6 }));
        // The top is never under the floor; a missing top is four times.
        assert_eq!(parse_band("2,1"), Some(Band { least: 2.0, most: 2.0, level: 0.8 }));
        assert_eq!(parse_band("0.7"), Some(Band { least: 0.7, most: 4.0, level: 0.8 }));
        assert_eq!(parse_band(""), None);
        assert_eq!(parse_band("x,1"), None);
    }

    #[test]
    fn roles_by_name() {
        assert_eq!(role(b"..CS_ScreenProbes_TemporalResolve_a911b49c_Wrapped.."), Some(Role::Resolve));
        assert_eq!(role(b"..CS_CacheScreenSpaceRadiance_eed2fc35.."), Some(Role::Radiance));
        assert_eq!(role(b"CS_AccumulateLocalLightInscatteringVolume_7e2f5eef"), Some(Role::Air));
        assert_eq!(role(b"CS_TemporalResolve"), None);
    }

    /// The passes compile with the system's compiler and the host takes them.
    #[test]
    fn the_passes_build() {
        let Some(host) = host() else {
            return;
        };
        make(&host.device, |bytes| host.uav_buffer(bytes)).expect("the passes");
    }

    const RT_STAGE_HLSL: &str = r#"
cbuffer Scene : register(b0) {
    uint width;
    uint height;
    uint enable_rt;
    uint pad0;
    float4 occluder_min;
    float4 occluder_max;
    float4 light_dir;
    float4 ambient_rgb;
};
struct RayRecord {
    float3 origin;
    float t_max;
    float3 dir;
    uint pixel;
};
#ifdef ACTIVATE
#define RS "RootConstants(num32BitConstants=20, b0), UAV(u0), UAV(u1)"
RWStructuredBuffer<RayRecord> rays : register(u0);
RWByteAddressBuffer indirect_args : register(u1);
[RootSignature(RS)]
[numthreads(8, 8, 1)]
void activate_rays(uint3 id : SV_DispatchThreadID) {
    if (id.x >= width || id.y >= height) return;
    uint idx = id.y * width + id.x;
    float2 uv = (float2(id.xy) + 0.5) / float2(width, height);
    float2 world_xy = (uv * 2.0 - 1.0) * float2(8.0, 4.5);
    RayRecord r;
    r.origin = float3(world_xy.x, world_xy.y, 10.0);
    r.t_max = occluder_min.w;
    r.dir = light_dir.xyz;
    r.pixel = idx;
    rays[idx] = r;
    if (idx == 0) {
        uint total = width * height;
        indirect_args.Store3(0, uint3((total + 63u) / 64u, 1u, 1u));
    }
}
#endif
#ifdef CLAMP
#define RS "SRV(t0), UAV(u0)"
ByteAddressBuffer src_args : register(t0);
RWByteAddressBuffer dst_args : register(u0);
[RootSignature(RS)]
[numthreads(1, 1, 1)]
void clamp_args() {
    dst_args.Store3(0, min(src_args.Load3(0), uint3(65535u, 65535u, 65535u)));
}
#endif
#ifdef TRACE
#define RS "RootConstants(num32BitConstants=20, b0), SRV(t0), UAV(u0)"
StructuredBuffer<RayRecord> rays : register(t0);
RWStructuredBuffer<float> visibility : register(u0);
[RootSignature(RS)]
[numthreads(64, 1, 1)]
void trace_rays(uint3 id : SV_DispatchThreadID) {
    uint total = width * height;
    if (id.x >= total) return;
    RayRecord r = rays[id.x];
    float3 inv_d = 1.0 / (abs(r.dir) < 1e-6 ? float3(1e-6, 1e-6, 1e-6) : r.dir);
    float3 t0 = (occluder_min.xyz - r.origin) * inv_d;
    float3 t1 = (occluder_max.xyz - r.origin) * inv_d;
    float3 tmin = min(t0, t1), tmax = max(t0, t1);
    float enter = max(max(tmin.x, tmin.y), max(tmin.z, 0.001));
    float exit_t = min(min(tmax.x, tmax.y), min(tmax.z, r.t_max));
    visibility[r.pixel] = (exit_t >= enter) ? 0.0 : 1.0;
}
#endif
#ifdef RT_RESOLVE
#define RS "RootConstants(num32BitConstants=20, b0), SRV(t0), DescriptorTable(UAV(u0))"
StructuredBuffer<float> visibility : register(t0);
RWTexture2D<uint> lighting : register(u0);
uint pack_r9g9b9e5(float3 c) {
    c = clamp(c, 0.0, 65408.0);
    float top = max(c.r, max(c.g, c.b));
    int e = clamp(int(floor(log2(max(top, 1e-9)))) + 16, 0, 31);
    float scale = exp2(float(24 - e));
    if (top * scale >= 511.5) { e = min(e + 1, 31); scale = exp2(float(24 - e)); }
    uint3 m = min(uint3(c * scale + 0.5), 511u);
    return m.r | (m.g << 9) | (m.b << 18) | (uint(e) << 27);
}
[RootSignature(RS)]
[numthreads(8, 8, 1)]
void resolve_rt(uint3 id : SV_DispatchThreadID) {
    if (id.x >= width || id.y >= height) return;
    uint idx = id.y * width + id.x;
    float vis = enable_rt != 0u ? visibility[idx] : 1.0;
    float3 rgb = ambient_rgb.rgb + float3(0.95, 0.90, 0.80) * light_dir.w * vis;
    lighting[id.xy] = pack_r9g9b9e5(rgb);
}
#endif
"#;

    const LEGACY_MEAN_RAISE_HLSL: &str = r#"
#define RS "RootConstants(num32BitConstants=16, b0), DescriptorTable(SRV(t0)), DescriptorTable(UAV(u0)), UAV(u1), UAV(u2), UAV(u3)"
#define CX 64
#define CY 36
#define CELLS (CX * CY)
#define PX 160
#define PY 90
cbuffer Set : register(b0) {
    float level; float constant; float strength; float radius;
    float near; float tan_y; uint width; uint height;
    float follow; float most; float air; uint probe;
    float jump; float fresh; float dt; float least;
};
Texture2D<float> depth : register(t0);
RWStructuredBuffer<float4> cells : register(u1);
RWStructuredBuffer<float4> points : register(u2);
RWByteAddressBuffer stats : register(u3);
static const float3 LUMA = float3(0.2126, 0.7152, 0.0722);
float3 seen(int2 p, float2 size) {
    float d = depth.Load(int3(clamp(p, int2(0, 0), int2(size) - 1), 0));
    float z = min(near / max(d, 1e-7), 1000.0);
    float2 ndc = (float2(p) + 0.5) / size * 2.0 - 1.0;
    return float3(ndc.x * tan_y * size.x / size.y * z, -ndc.y * tan_y * z, z);
}
#ifdef MEAN
[RootSignature(RS)]
[numthreads(1, 1, 1)]
void mean() {
    float3 sum = 0;
    for (uint i = 0; i < CELLS; i++) sum += cells[i].rgb;
    float4 was = cells[CELLS];
    float3 now = sum / CELLS;
    float a = max(dot(now, LUMA), 1e-5), b = max(dot(was.rgb, LUMA), 1e-5);
    bool jumped = jump > 0.0 && max(a / b, b / a) > jump;
    float age = was.a > 0.0 && !jumped ? was.a + dt : 0.001;
    cells[CELLS] = float4(was.a > 0.0 ? lerp(was.rgb, now, jumped ? 1.0 : follow) : now, age);
}
#endif
#ifdef RAISE
RWTexture2D<uint> light : register(u0);
float3 unpack(uint v) {
    return float3(v & 511, (v >> 9) & 511, (v >> 18) & 511) * exp2(float(v >> 27) - 24.0);
}
uint pack(float3 c) {
    c = clamp(c, 0.0, 65408.0);
    float top = max(c.r, max(c.g, c.b));
    int e = clamp(int(floor(log2(max(top, 1e-9)))) + 16, 0, 31);
    float scale = exp2(float(24 - e));
    if (top * scale >= 511.5) { e = min(e + 1, 31); scale = exp2(float(24 - e)); }
    uint3 m = min(uint3(c * scale + 0.5), 511u);
    return m.r | (m.g << 9) | (m.b << 18) | (uint(e) << 27);
}
[RootSignature(RS)]
[numthreads(8, 8, 1)]
void raise(uint3 at : SV_DispatchThreadID) {
    float2 size = float2(width, height);
    if (at.x >= width || at.y >= height) return;
    int2 p = int2(at.xy);
    float3 c = seen(p, size);
    float3 l = seen(p + int2(-1, 0), size), r = seen(p + int2(1, 0), size);
    float3 u = seen(p + int2(0, -1), size), d = seen(p + int2(0, 1), size);
    float3 dx = abs(l.z - c.z) < abs(r.z - c.z) ? c - l : r - c;
    float3 dy = abs(u.z - c.z) < abs(d.z - c.z) ? c - u : d - c;
    float3 n = normalize(cross(dy, dx) + float3(0, 0, -1e-9));
    if (dot(n, c) > 0.0) n = -n;
    float open = 1.0;
    float px = min(radius * size.y / (2.0 * tan_y * c.z), 128.0);
    float noise = frac(52.9829189 * frac(dot(float2(p), float2(0.06711056, 0.00583715))));
    if (strength > 0.0 && px >= 1.5 && c.z < 400.0) {
        float sum = 0.0;
        const int N = 12;
        for (int i = 0; i < N; i++) {
            float a = i * 2.399963 + noise * 6.2831853;
            float2 o = float2(cos(a), sin(a)) * (sqrt((i + 0.5) / N) * px);
            float3 v = seen(p + int2(round(o)), size) - c;
            float vv = dot(v, v);
            float over = dot(v, n) - 0.004 * c.z - 0.005;
            sum += max(over, 0.0) / sqrt(vv + 1e-6) * saturate(1.0 - vv / (radius * radius));
        }
        open = saturate(1.0 - strength * 2.0 * sum / N);
    }
    float3 found;
    if (constant > 0.0) {
        found = float3(1.0, 0.98, 0.94) * constant * open;
        light[p] = pack(found);
        return;
    }
    float2 g = (float2(p) + 0.5) / size * float2(PX, PY) - 0.5 + (noise - 0.5) * 0.5;
    int2 g0 = int2(floor(g));
    float2 f = g - float2(g0);
    float4 sr = 0, sg = 0, sb = 0;
    float sum = 0;
    for (int k = 0; k < 4; k++) {
        int2 q = clamp(g0 + int2(k & 1, k >> 1), int2(0, 0), int2(PX, PY) - 1);
        float z = seen(int2((float2(q) + 0.5) * size / float2(PX, PY)), size).z;
        float2 b = float2(k & 1 ? f.x : 1 - f.x, k >> 1 ? f.y : 1 - f.y);
        float w = (b.x * b.y + 1e-3) / (0.02 + abs(z - c.z) / c.z);
        uint at0 = (q.y * PX + q.x) * 3;
        sr += w * points[at0];
        sg += w * points[at0 + 1];
        sb += w * points[at0 + 2];
        sum += w;
    }
    float3 all = float3(sr.w, sg.w, sb.w) / sum;
    found = all + 1.2 * float3(dot(sr.xyz, n), dot(sg.xyz, n), dot(sb.xyz, n)) / sum;
    found = clamp(found, 0.2 * all, 2.5 * all);
    found = lerp(found, cells[CELLS].rgb, 0.2) * level * open;
    float3 was = unpack(light[p]);
    float left = dot(was, LUMA);
    if (was.g < 0.5 * min(was.r, was.b) && max(was.r, was.b) > 0.02) was = 0.0;
    float bound = fresh > 0.0 ? lerp(1.0, most, saturate(cells[CELLS].a / fresh)) : most;
    float top = max(dot(found, LUMA) * bound, 1e-6);
    float has = dot(was, LUMA);
    was *= min(1.0, top / max(has, 1e-6));
    float3 written = max(was, found * least);
    light[p] = pack(written);
}
#endif
"#;

    fn unpack_r9g9b9e5(v: u32) -> [f32; 3] {
        let scale = 2.0f32.powi((v >> 27) as i32 - 24);
        [
            (v & 511) as f32 * scale,
            ((v >> 9) & 511) as f32 * scale,
            ((v >> 18) & 511) as f32 * scale,
        ]
    }

    struct FullFrameRun {
        adapter: String,
        /// `[rt_ms, keep_ms, mean_ms, place_gather_ms, raise_ms, total_gpu_ms]`
        gpu_p50: [f64; 6],
        gpu_p95: [f64; 6],
        frame_p50: f64,
        frame_p95: f64,
        pixels: Vec<u32>,
    }

    fn run_full_rt_ambient_frame(
        width: u32,
        height: u32,
        enable_rt: bool,
        occluder_right: bool,
        use_legacy_mean_raise: bool,
        warmups: usize,
        iters: usize,
        read_pixels: bool,
    ) -> Option<FullFrameRun> {
        use std::mem::ManuallyDrop;
        use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0;
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_FORMAT, DXGI_FORMAT_R11G11B10_FLOAT, DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_R32_UINT,
            DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
        };
        use windows::Win32::Graphics::Dxgi::{
            CreateDXGIFactory2, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_CREATE_FACTORY_FLAGS,
            IDXGIAdapter1, IDXGIFactory4,
        };
        use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
        use windows::core::{Interface, PCSTR};

        let force_warp = std::env::var_os("VICTORY_D3D12_WARP").is_some_and(|v| v == "1");
        let (device, adapter_name) = unsafe {
            let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).ok()?;
            let mut found = None;
            if !force_warp {
                for i in 0.. {
                    let Ok(adapter) = factory.EnumAdapters1(i) else { break };
                    let Ok(desc) = adapter.GetDesc1() else { continue };
                    if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                        continue;
                    }
                    let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
                    let name = String::from_utf16_lossy(&desc.Description[..len]);
                    let mut dev: Option<ID3D12Device> = None;
                    if D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_12_0, &mut dev).is_ok()
                        && let Some(d) = dev
                    {
                        found = Some((d, name));
                        break;
                    }
                }
            }
            match found {
                Some(pair) => pair,
                None => {
                    let warp: IDXGIAdapter1 = factory.EnumWarpAdapter().ok()?;
                    let mut dev: Option<ID3D12Device> = None;
                    D3D12CreateDevice(&warp, D3D_FEATURE_LEVEL_12_0, &mut dev).ok()?;
                    (dev?, "WARP".to_owned())
                }
            }
        };

        let make_buf = |bytes: u64, heap: D3D12_HEAP_TYPE, flags: D3D12_RESOURCE_FLAGS, state: D3D12_RESOURCE_STATES| -> Option<ID3D12Resource> {
            let mut res: Option<ID3D12Resource> = None;
            unsafe {
                device
                    .CreateCommittedResource(
                        &D3D12_HEAP_PROPERTIES { Type: heap, ..Default::default() },
                        D3D12_HEAP_FLAG_NONE,
                        &D3D12_RESOURCE_DESC {
                            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                            Width: bytes.max(256),
                            Height: 1,
                            DepthOrArraySize: 1,
                            MipLevels: 1,
                            Format: DXGI_FORMAT_UNKNOWN,
                            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                            Flags: flags,
                            ..Default::default()
                        },
                        state,
                        None,
                        &mut res,
                    )
                    .ok()?;
            }
            res
        };
        let make_tex = |w: u32, h: u32, format: DXGI_FORMAT, flags: D3D12_RESOURCE_FLAGS, state: D3D12_RESOURCE_STATES| -> Option<ID3D12Resource> {
            let mut res: Option<ID3D12Resource> = None;
            unsafe {
                device
                    .CreateCommittedResource(
                        &D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() },
                        D3D12_HEAP_FLAG_NONE,
                        &D3D12_RESOURCE_DESC {
                            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                            Width: u64::from(w),
                            Height: h,
                            DepthOrArraySize: 1,
                            MipLevels: 1,
                            Format: format,
                            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
                            Flags: flags,
                            ..Default::default()
                        },
                        state,
                        None,
                        &mut res,
                    )
                    .ok()?;
            }
            res
        };
        let make_stage = |src: &str, def: &str, entry: PCSTR| -> (ID3D12RootSignature, ID3D12PipelineState) {
            let code = crate::host::compile(&format!("#define {def}\n{src}"), entry, s!("cs_5_1")).unwrap();
            unsafe {
                let root: ID3D12RootSignature = device.CreateRootSignature(0, &code).unwrap();
                let pso = device
                    .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                        pRootSignature: std::mem::transmute_copy(&root),
                        CS: D3D12_SHADER_BYTECODE { pShaderBytecode: code.as_ptr().cast(), BytecodeLength: code.len() },
                        ..Default::default()
                    })
                    .unwrap();
                (root, pso)
            }
        };

        let pass = make(&device, |bytes| {
            make_buf(bytes, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COMMON)
        })
        .ok()?;
        let legacy_mean = use_legacy_mean_raise.then(|| make_stage(LEGACY_MEAN_RAISE_HLSL, "MEAN", s!("mean")).1);
        let legacy_raise = use_legacy_mean_raise.then(|| make_stage(LEGACY_MEAN_RAISE_HLSL, "RAISE", s!("raise")).1);

        let activate = make_stage(RT_STAGE_HLSL, "ACTIVATE", s!("activate_rays"));
        let clamp = make_stage(RT_STAGE_HLSL, "CLAMP", s!("clamp_args"));
        let trace = make_stage(RT_STAGE_HLSL, "TRACE", s!("trace_rays"));
        let rt_resolve = make_stage(RT_STAGE_HLSL, "RT_RESOLVE", s!("resolve_rt"));

        unsafe {
            let queue: ID3D12CommandQueue = device
                .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() })
                .ok()?;
            let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).ok()?;
            let list: ID3D12GraphicsCommandList = device
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)
                .ok()?;
            list.Close().ok()?;
            let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).ok()?;
            let event = CreateEventW(None, false, false, None).ok()?;
            let ts_freq = queue.GetTimestampFrequency().unwrap_or(1).max(1);

            let arg = D3D12_INDIRECT_ARGUMENT_DESC { Type: D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH, ..Default::default() };
            let mut dispatch_sig: Option<ID3D12CommandSignature> = None;
            device
                .CreateCommandSignature(
                    &D3D12_COMMAND_SIGNATURE_DESC { ByteStride: 12, NumArgumentDescs: 1, pArgumentDescs: &arg, NodeMask: 0 },
                    None::<&ID3D12RootSignature>,
                    &mut dispatch_sig,
                )
                .ok()?;
            let dispatch_sig = dispatch_sig?;

            let pixels = u64::from(width) * u64::from(height);
            let rays = make_buf(pixels * 32, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COMMON)?;
            let raw_args = make_buf(256, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COMMON)?;
            let clamped_args = make_buf(256, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COMMON)?;
            let visibility = make_buf(pixels * 4, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COMMON)?;

            let depth_tex = make_tex(width, height, DXGI_FORMAT_R32_FLOAT, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COPY_DEST)?;
            let rad_tex = make_tex(640, 360, DXGI_FORMAT_R11G11B10_FLOAT, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_DEST)?;
            let light_tex = make_tex(width, height, DXGI_FORMAT_R32_UINT, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_UNORDERED_ACCESS)?;

            let row_pitch = (width * 4).next_multiple_of(256);
            let rad_pitch = (640u32 * 4).next_multiple_of(256);
            let depth_up = make_buf(u64::from(row_pitch) * u64::from(height), D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_GENERIC_READ)?;
            let rad_up = make_buf(u64::from(rad_pitch) * 360, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_GENERIC_READ)?;
            let cells_zero = make_buf(CELL_BYTES, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_GENERIC_READ)?;
            let readback = make_buf(u64::from(row_pitch) * u64::from(height), D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COPY_DEST)?;

            let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
            depth_up.Map(0, None, Some(&mut ptr)).ok()?;
            for y in 0..height {
                let row = ptr.cast::<u8>().add((y * row_pitch) as usize).cast::<f32>();
                for x in 0..width {
                    let occ = if occluder_right {
                        x >= (width * 9) / 16 && x < (width * 13) / 16 && y >= height / 4 && y < (3 * height) / 4
                    } else {
                        x >= (width * 3) / 16 && x < (width * 7) / 16 && y >= height / 4 && y < (3 * height) / 4
                    };
                    let z = if occ { 3.5f32 } else { 10.0 + (x as f32 / width as f32) * 6.0 };
                    *row.add(x as usize) = 0.1 / z;
                }
            }
            depth_up.Unmap(0, None);

            rad_up.Map(0, None, Some(&mut ptr)).ok()?;
            for y in 0..360u32 {
                let row = ptr.cast::<u8>().add((y * rad_pitch) as usize).cast::<u32>();
                for x in 0..640u32 {
                    *row.add(x as usize) = 0x380u32 | (0x360u32 << 11) | (0x1a0u32 << 22);
                }
            }
            rad_up.Unmap(0, None);

            cells_zero.Map(0, None, Some(&mut ptr)).ok()?;
            std::ptr::write_bytes(ptr.cast::<u8>(), 0, CELL_BYTES as usize);
            cells_zero.Unmap(0, None);

            let heap: ID3D12DescriptorHeap = device
                .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    NumDescriptors: 3,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                })
                .ok()?;
            let inc = device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
            let cpu = |i: u32| D3D12_CPU_DESCRIPTOR_HANDLE { ptr: heap.GetCPUDescriptorHandleForHeapStart().ptr + (i * inc) as usize };
            let gpu = |i: u32| D3D12_GPU_DESCRIPTOR_HANDLE { ptr: heap.GetGPUDescriptorHandleForHeapStart().ptr + u64::from(i * inc) };

            device.CreateShaderResourceView(
                &depth_tex,
                Some(&D3D12_SHADER_RESOURCE_VIEW_DESC {
                    Format: DXGI_FORMAT_R32_FLOAT,
                    ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                    Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                    Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_SRV { MipLevels: 1, ..Default::default() } },
                }),
                cpu(0),
            );
            device.CreateUnorderedAccessView(
                &light_tex,
                None,
                Some(&D3D12_UNORDERED_ACCESS_VIEW_DESC {
                    Format: DXGI_FORMAT_R32_UINT,
                    ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_UAV { MipSlice: 0, PlaneSlice: 0 } },
                }),
                cpu(1),
            );
            device.CreateUnorderedAccessView(
                &rad_tex,
                None,
                Some(&D3D12_UNORDERED_ACCESS_VIEW_DESC {
                    Format: DXGI_FORMAT_R11G11B10_FLOAT,
                    ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_UAV { MipSlice: 0, PlaneSlice: 0 } },
                }),
                cpu(2),
            );

            let mut ts_heap: Option<ID3D12QueryHeap> = None;
            device
                .CreateQueryHeap(
                    &D3D12_QUERY_HEAP_DESC { Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP, Count: 8, NodeMask: 0 },
                    &mut ts_heap,
                )
                .ok()?;
            let ts_heap = ts_heap?;
            let ts_rb = make_buf(64, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COPY_DEST)?;

            let mut fence_val = 0u64;
            let mut wait = || {
                fence_val += 1;
                queue.Signal(&fence, fence_val).unwrap();
                fence.SetEventOnCompletion(fence_val, event).unwrap();
                WaitForSingleObject(event, INFINITE);
            };
            let footprint = |res: &ID3D12Resource, w: u32, h: u32, pitch: u32, format: DXGI_FORMAT| D3D12_TEXTURE_COPY_LOCATION {
                pResource: ManuallyDrop::new(Some(res.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                        Offset: 0,
                        Footprint: D3D12_SUBRESOURCE_FOOTPRINT { Format: format, Width: w, Height: h, Depth: 1, RowPitch: pitch },
                    },
                },
            };
            let sub0 = |res: &ID3D12Resource| D3D12_TEXTURE_COPY_LOCATION {
                pResource: ManuallyDrop::new(Some(res.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
            };

            allocator.Reset().unwrap();
            list.Reset(&allocator, None).unwrap();
            list.CopyTextureRegion(&sub0(&depth_tex), 0, 0, 0, &footprint(&depth_up, width, height, row_pitch, DXGI_FORMAT_R32_FLOAT), None);
            list.CopyTextureRegion(&sub0(&rad_tex), 0, 0, 0, &footprint(&rad_up, 640, 360, rad_pitch, DXGI_FORMAT_R11G11B10_FLOAT), None);
            list.CopyBufferRegion(&pass.cells, 0, &cells_zero, 0, CELL_BYTES);
            list.ResourceBarrier(&[
                crate::host::transition(&depth_tex, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE),
                crate::host::transition(&rad_tex, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS),
                crate::host::transition(&pass.cells, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COMMON),
            ]);
            list.Close().unwrap();
            let cmd: ID3D12CommandList = list.cast().unwrap();
            queue.ExecuteCommandLists(&[Some(cmd)]);
            wait();

            let (occ_min, occ_max) = if occluder_right {
                ([1.0f32, -2.25, 4.0], [5.0f32, 2.25, 5.0])
            } else {
                ([-5.0f32, -2.25, 4.0], [-1.0f32, 2.25, 5.0])
            };
            let scene: [u32; 20] = [
                width, height, u32::from(enable_rt), 0,
                occ_min[0].to_bits(), occ_min[1].to_bits(), occ_min[2].to_bits(), 20.0f32.to_bits(),
                occ_max[0].to_bits(), occ_max[1].to_bits(), occ_max[2].to_bits(), 0,
                0.0f32.to_bits(), 0.0f32.to_bits(), (-1.0f32).to_bits(), 2.0f32.to_bits(),
                0.08f32.to_bits(), 0.08f32.to_bits(), 0.08f32.to_bits(), 0,
            ];
            let mut amb_words = [0u32; 16];
            for (to, v) in amb_words.iter_mut().zip([0.8f32, 0.0, 1.0, 0.6, 0.1, 0.45]) {
                *to = v.to_bits();
            }
            amb_words[6] = width;
            amb_words[7] = height;
            amb_words[8] = 0.04f32.to_bits();
            amb_words[9] = 4.0f32.to_bits();
            amb_words[15] = 0.25f32.to_bits();
            amb_words[14] = 0.033f32.to_bits();

            let raise_group = if use_legacy_mean_raise { 8u32 } else { RAISE_GROUP };
            let mut run_one = |do_readback: bool| -> ([f64; 6], f64, Vec<u32>) {
                let wall_start = std::time::Instant::now();
                allocator.Reset().unwrap();
                list.Reset(&allocator, None).unwrap();
                let heaps = [Some(heap.clone())];
                list.SetDescriptorHeaps(&heaps);

                list.EndQuery(&ts_heap, D3D12_QUERY_TYPE_TIMESTAMP, 0);
                if enable_rt {
                    list.SetComputeRootSignature(&activate.0);
                    list.SetPipelineState(&activate.1);
                    list.SetComputeRoot32BitConstants(0, 20, scene.as_ptr().cast(), 0);
                    list.SetComputeRootUnorderedAccessView(1, rays.GetGPUVirtualAddress());
                    list.SetComputeRootUnorderedAccessView(2, raw_args.GetGPUVirtualAddress());
                    list.Dispatch(width.div_ceil(8), height.div_ceil(8), 1);

                    list.ResourceBarrier(&[
                        crate::host::transition(&raw_args, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE),
                        crate::host::transition(&clamped_args, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_UNORDERED_ACCESS),
                    ]);
                    list.SetComputeRootSignature(&clamp.0);
                    list.SetPipelineState(&clamp.1);
                    list.SetComputeRootShaderResourceView(0, raw_args.GetGPUVirtualAddress());
                    list.SetComputeRootUnorderedAccessView(1, clamped_args.GetGPUVirtualAddress());
                    list.Dispatch(1, 1, 1);

                    list.ResourceBarrier(&[
                        crate::host::transition(&rays, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE),
                        crate::host::transition(&clamped_args, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT),
                    ]);
                    list.SetComputeRootSignature(&trace.0);
                    list.SetPipelineState(&trace.1);
                    list.SetComputeRoot32BitConstants(0, 20, scene.as_ptr().cast(), 0);
                    list.SetComputeRootShaderResourceView(1, rays.GetGPUVirtualAddress());
                    list.SetComputeRootUnorderedAccessView(2, visibility.GetGPUVirtualAddress());
                    list.ExecuteIndirect(&dispatch_sig, 1, &clamped_args, 0, None, 0);

                    list.ResourceBarrier(&[
                        crate::host::transition(&visibility, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE),
                        crate::host::transition(&raw_args, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COMMON),
                        crate::host::transition(&rays, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COMMON),
                        crate::host::transition(&clamped_args, D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT, D3D12_RESOURCE_STATE_COMMON),
                    ]);
                }

                list.SetComputeRootSignature(&rt_resolve.0);
                list.SetPipelineState(&rt_resolve.1);
                list.SetComputeRoot32BitConstants(0, 20, scene.as_ptr().cast(), 0);
                list.SetComputeRootShaderResourceView(1, visibility.GetGPUVirtualAddress());
                list.SetComputeRootDescriptorTable(2, gpu(1));
                list.Dispatch(width.div_ceil(8), height.div_ceil(8), 1);
                if enable_rt {
                    list.ResourceBarrier(&[crate::host::transition(&visibility, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COMMON)]);
                }
                uav_barrier(&list);
                list.EndQuery(&ts_heap, D3D12_QUERY_TYPE_TIMESTAMP, 1);

                list.SetComputeRootSignature(&pass.root);
                list.SetComputeRoot32BitConstants(0, 16, amb_words.as_ptr().cast(), 0);
                list.SetComputeRootDescriptorTable(1, gpu(0));
                list.SetComputeRootDescriptorTable(2, gpu(2));
                list.SetComputeRootUnorderedAccessView(3, pass.cells.GetGPUVirtualAddress());
                list.SetComputeRootUnorderedAccessView(4, pass.points.GetGPUVirtualAddress());
                list.SetComputeRootUnorderedAccessView(5, pass.stats.GetGPUVirtualAddress());
                list.SetPipelineState(&pass.keep);
                list.Dispatch(8, 9, 1);
                uav_barrier(&list);
                list.EndQuery(&ts_heap, D3D12_QUERY_TYPE_TIMESTAMP, 2);

                list.SetPipelineState(legacy_mean.as_ref().unwrap_or(&pass.mean));
                list.Dispatch(1, 1, 1);
                uav_barrier(&list);
                list.EndQuery(&ts_heap, D3D12_QUERY_TYPE_TIMESTAMP, 3);

                list.SetComputeRootDescriptorTable(2, gpu(1));
                list.SetPipelineState(&pass.place);
                list.Dispatch(8, 9, 1);
                uav_barrier(&list);
                list.SetPipelineState(&pass.gather);
                list.Dispatch(20, 45, 1);
                uav_barrier(&list);
                list.EndQuery(&ts_heap, D3D12_QUERY_TYPE_TIMESTAMP, 4);

                list.SetPipelineState(legacy_raise.as_ref().unwrap_or(&pass.raise));
                list.Dispatch(width.div_ceil(raise_group), height.div_ceil(raise_group), 1);
                uav_barrier(&list);
                list.EndQuery(&ts_heap, D3D12_QUERY_TYPE_TIMESTAMP, 5);

                list.ResolveQueryData(&ts_heap, D3D12_QUERY_TYPE_TIMESTAMP, 0, 6, &ts_rb, 0);
                if do_readback {
                    list.ResourceBarrier(&[crate::host::transition(&light_tex, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
                    list.CopyTextureRegion(&footprint(&readback, width, height, row_pitch, DXGI_FORMAT_R32_UINT), 0, 0, 0, &sub0(&light_tex), None);
                    list.ResourceBarrier(&[crate::host::transition(&light_tex, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS)]);
                }
                list.Close().unwrap();
                let cmd: ID3D12CommandList = list.cast().unwrap();
                queue.ExecuteCommandLists(&[Some(cmd)]);
                wait();
                let wall_ms = wall_start.elapsed().as_secs_f64() * 1000.0;

                let mut ts = [0u64; 6];
                ts_rb.Map(0, None, Some(&mut ptr)).unwrap();
                std::ptr::copy_nonoverlapping(ptr.cast::<u64>(), ts.as_mut_ptr(), 6);
                ts_rb.Unmap(0, None);
                let ms = |a: u64, b: u64| b.saturating_sub(a) as f64 * 1000.0 / ts_freq as f64;
                let gpu = [
                    ms(ts[0], ts[1]),
                    ms(ts[1], ts[2]),
                    ms(ts[2], ts[3]),
                    ms(ts[3], ts[4]),
                    ms(ts[4], ts[5]),
                    ms(ts[0], ts[5]),
                ];

                let mut out = Vec::new();
                if do_readback {
                    out.resize((width * height) as usize, 0);
                    readback.Map(0, None, Some(&mut ptr)).unwrap();
                    for y in 0..height as usize {
                        let row = ptr.cast::<u8>().add(y * row_pitch as usize).cast::<u32>();
                        std::ptr::copy_nonoverlapping(row, out.as_mut_ptr().add(y * width as usize), width as usize);
                    }
                    readback.Unmap(0, None);
                }
                (gpu, wall_ms, out)
            };

            for _ in 0..warmups {
                run_one(false);
            }
            let mut cols: [Vec<f64>; 6] = std::array::from_fn(|_| Vec::with_capacity(iters));
            let mut walls = Vec::with_capacity(iters);
            for _ in 0..iters {
                let (g, w, _) = run_one(false);
                for k in 0..6 {
                    cols[k].push(g[k]);
                }
                walls.push(w);
            }
            let (_, _, pixels) = if read_pixels { run_one(true) } else { ([0.0; 6], 0.0, Vec::new()) };

            let pct = |mut v: Vec<f64>, p: f64| -> f64 {
                v.sort_by(|a, b| a.total_cmp(b));
                let idx = ((v.len() as f64 * p).ceil() as usize).saturating_sub(1).min(v.len() - 1);
                v[idx]
            };
            Some(FullFrameRun {
                adapter: adapter_name,
                gpu_p50: std::array::from_fn(|k| pct(cols[k].clone(), 0.50)),
                gpu_p95: std::array::from_fn(|k| pct(cols[k].clone(), 0.95)),
                frame_p50: pct(walls.clone(), 0.50),
                frame_p95: pct(walls, 0.95),
                pixels,
            })
        }
    }

    /// End-to-end verification from `ActivateRays` through indirect `RayTrace`,
    /// RT resolve, and production `ambient` (`keep -> mean -> place -> gather -> raise`):
    /// 1. Moving the occluder shifts the shadow in the final `R9G9B9E5` output.
    /// 2. Parallel `mean` (64 threads) + 32x32 `raise` produce bit-exact output
    ///    against the legacy 1-thread `mean` + 8x8 `raise`.
    #[test]
    fn rt_to_ambient_resolve_movable_occluder_and_bit_exact_parity() {
        let Some(left_opt) = run_full_rt_ambient_frame(320, 180, true, false, false, 1, 1, true)
        else {
            return;
        };
        let right_opt = run_full_rt_ambient_frame(320, 180, true, true, false, 1, 1, true).unwrap();
        let left_legacy = run_full_rt_ambient_frame(320, 180, true, false, true, 1, 1, true).unwrap();

        let luma = |px: &[u32], x: u32, y: u32| -> f32 {
            let rgb = unpack_r9g9b9e5(px[(y * 320 + x) as usize]);
            0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
        };
        let (l_a, r_a) = (luma(&left_opt.pixels, 96, 90), luma(&left_opt.pixels, 224, 90));
        let (l_b, r_b) = (luma(&right_opt.pixels, 96, 90), luma(&right_opt.pixels, 224, 90));
        assert!(l_a * 2.0 < r_a, "occluder left should shadow left ({l_a}) vs right ({r_a})");
        assert!(r_b * 2.0 < l_b, "occluder right should shadow right ({r_b}) vs left ({l_b})");
        assert_ne!(left_opt.pixels, right_opt.pixels, "moving occluder must change output");
        assert_eq!(
            left_opt.pixels, left_legacy.pixels,
            "32x32 raise + parallel mean must be bit-exact with legacy 8x8 raise + 1-thread mean"
        );
    }

    /// Full-frame 1080p and 4K RT OFF vs RT ON + per-pass benchmark (`rt`, `mean`,
    /// `raise`, full-frame GPU and wall-clock p50/p95 against 33.333 ms = 30 FPS).
    /// Run with:
    /// `cargo test -p victory-gpu --release --lib ambient::tests::rt_ambient_full_frame_bench -- --exact --ignored --nocapture --test-threads=1`
    #[test]
    #[ignore]
    fn rt_ambient_full_frame_bench() {
        const BUDGET_MS: f64 = 33.333;
        const WARMUP: usize = 24;
        const ITERS: usize = 60;
        let to_bytes = |words: &[u32]| -> &[u8] {
            // SAFETY: `&[u32]` has valid initialized bytes of length `words.len() * 4`.
            unsafe { std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), std::mem::size_of_val(words)) }
        };
        for (w, h) in [(1920, 1080), (3840, 2160)] {
            let rt_off = run_full_rt_ambient_frame(w, h, false, false, false, WARMUP, ITERS, false).unwrap();
            let rt_on = run_full_rt_ambient_frame(w, h, true, false, false, WARMUP, ITERS, true).unwrap();
            let rt_on_right = run_full_rt_ambient_frame(w, h, true, true, false, 2, 2, true).unwrap();
            assert_ne!(rt_on.pixels, rt_on_right.pixels, "moving occluder must change output");
            let hash_left = blake3::hash(to_bytes(&rt_on.pixels)).to_hex();
            let hash_right = blake3::hash(to_bytes(&rt_on_right.pixels)).to_hex();
            println!(
                "adapter={} res={w}x{h} rays={} raise_group={RAISE_GROUP} budget_30fps={BUDGET_MS:.3}ms",
                rt_on.adapter,
                u64::from(w) * u64::from(h)
            );
            println!(
                "RT_OFF  gpu_p50/p95={:.4}/{:.4}ms ({:.2}% of 33.3ms) wall_p50/p95={:.3}/{:.3}ms | mean={:.4}ms raise={:.4}ms",
                rt_off.gpu_p50[5], rt_off.gpu_p95[5], rt_off.gpu_p50[5] / BUDGET_MS * 100.0,
                rt_off.frame_p50, rt_off.frame_p95, rt_off.gpu_p50[2], rt_off.gpu_p50[4]
            );
            println!(
                "RT_ON   gpu_p50/p95={:.4}/{:.4}ms ({:.2}% of 33.3ms) wall_p50/p95={:.3}/{:.3}ms | rt={:.4}ms keep={:.4}ms mean={:.4}ms pg={:.4}ms raise={:.4}ms",
                rt_on.gpu_p50[5], rt_on.gpu_p95[5], rt_on.gpu_p50[5] / BUDGET_MS * 100.0,
                rt_on.frame_p50, rt_on.frame_p95,
                rt_on.gpu_p50[0], rt_on.gpu_p50[1], rt_on.gpu_p50[2], rt_on.gpu_p50[3], rt_on.gpu_p50[4]
            );
            println!(
                "RT_OVERHEAD delta_gpu={:+.4}ms ({:.2}% of 33.3ms) | hashes left={} right={}",
                rt_on.gpu_p50[5] - rt_off.gpu_p50[5],
                (rt_on.gpu_p50[5] - rt_off.gpu_p50[5]) / BUDGET_MS * 100.0,
                &hash_left[..16],
                &hash_right[..16]
            );
        }
    }
}
