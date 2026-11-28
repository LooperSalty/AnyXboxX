//! Opt-in mip-tail allocation policy and the descriptor rebinds it can require.
//!
//! `VICTORY_TEXTURE_AUTO_MIP_CLAMP` is off by default. Raising the host base
//! and rebasing SRVs is lossy: shaders may see different mips in `Load(.., mip)`,
//! different sizes from `GetDimensions`, and different results in texel-size math.

use std::collections::HashSet;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D12::{D3D12_RESOURCE_STATE_COPY_DEST, ID3D12Resource};

use super::{
    CAP_LIFTED, KINDS, Made, Resource, ResourceDesc, alias_of, element, fills, mip_count,
    pc_format, resource, resource_release, texture_layout,
};
use crate::object::{This, add_ref};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MipClampMode {
    Off,
    Auto,
    MaxDim(u32),
}

fn parse_mip_clamp_mode(raw: Option<&str>) -> MipClampMode {
    let Some(value) = raw.map(str::trim) else {
        return MipClampMode::Off;
    };
    if ["1", "true", "auto", "on"].iter().any(|on| value.eq_ignore_ascii_case(on)) {
        return MipClampMode::Auto;
    }
    match value.parse::<u32>() {
        Ok(dim) if dim >= 4 => MipClampMode::MaxDim(dim),
        _ => MipClampMode::Off,
    }
}

#[cfg(test)]
thread_local! {
    static TEST_MODE: std::cell::Cell<Option<MipClampMode>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn test_mode(raw: Option<&str>) -> impl Drop {
    struct Restore(Option<MipClampMode>);

    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_MODE.set(self.0);
        }
    }

    Restore(TEST_MODE.replace(Some(parse_mip_clamp_mode(raw))))
}

fn mip_clamp_mode() -> MipClampMode {
    #[cfg(test)]
    if let Some(mode) = TEST_MODE.get() {
        return mode;
    }
    static MODE: OnceLock<MipClampMode> = OnceLock::new();
    *MODE.get_or_init(|| {
        let value = std::env::var("VICTORY_TEXTURE_AUTO_MIP_CLAMP").ok();
        parse_mip_clamp_mode(value.as_deref())
    })
}

pub(crate) fn enabled() -> bool {
    mip_clamp_mode() != MipClampMode::Off
}

pub(super) fn budget_override_bytes() -> Option<u64> {
    static LIMIT: OnceLock<Option<u64>> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        let mb = std::env::var("VICTORY_TEXTURE_BUDGET_MB").ok()?;
        mb.trim().parse::<u64>().ok().map(|mb| mb << 20)
    })
}

fn auto_mip_clamp_target_dim(
    mode: MipClampMode,
    video_memory: Option<[u64; 4]>,
    extra_reserved: u64,
    filled_bytes: u64,
    budget_override: Option<u64>,
) -> Option<u32> {
    const GIB_8: u64 = 8 << 30;
    const GIB_12: u64 = 12 << 30;
    let pressured = |used, budget| u128::from(used) * 100 >= u128::from(budget) * 85;
    match mode {
        MipClampMode::Off => None,
        MipClampMode::MaxDim(dim) => Some(dim.max(4)),
        MipClampMode::Auto => {
            if let Some(limit) = budget_override {
                if limit <= GIB_8 || pressured(filled_bytes, limit) {
                    return Some(1024);
                }
                if limit <= GIB_12 {
                    return Some(2048);
                }
            }
            if let Some([budget, used, ..]) = video_memory
                && budget > 0
            {
                if budget <= GIB_8 || pressured(used.saturating_add(extra_reserved), budget) {
                    return Some(1024);
                }
                return (budget <= GIB_12).then_some(2048);
            }
            pressured(filled_bytes, 4 << 30).then_some(1024)
        }
    }
}

fn clamp_eligible(desc: &ResourceDesc, placed: bool, init_state: u32) -> bool {
    const TEXTURE_2D: u32 = 3;
    fills(desc)
        && desc.dimension == TEXTURE_2D
        && desc.depth_or_array_size == 1
        && desc.sample_count <= 1
        && (placed || crate::host::texture_state(init_state) != D3D12_RESOURCE_STATE_COPY_DEST.0 as u32)
}

fn compute_mip_clamp_base(desc: &ResourceDesc, max_dim: u32) -> u32 {
    let Ok(width) = u32::try_from(desc.width) else {
        return 0;
    };
    let height = desc.height;
    let max_dim = max_dim.max(4);
    let total_mips = mip_count(desc);
    if total_mips <= 1 || width.max(height) <= max_dim {
        return 0;
    }
    let Some(layout) = texture_layout(desc).filter(|t| t.planes.len() == 1) else {
        return 0;
    };
    let layout = &layout.planes[0].layout;
    if layout.desc.swizzle.xor() {
        return 0;
    }
    let Some((_, block)) = element(pc_format(desc.format)) else {
        return 0;
    };
    let block = block as u32;
    let last = layout.first_tail_mip.min(total_mips - 1).min(31);
    let mut base = 0;
    while base < last && (width >> base).max(height >> base) > max_dim {
        base += 1;
    }
    while base > 0 {
        let w = (width >> base).max(1);
        let h = (height >> base).max(1);
        if w >= block && h >= block && w.is_multiple_of(block) && h.is_multiple_of(block) {
            break;
        }
        base -= 1;
    }
    base
}

pub(super) fn mip_clamp_for(this: This, r: &Resource) -> u32 {
    let mode = mip_clamp_mode();
    if mode == MipClampMode::Off
        || r.host.whole.load(Relaxed)
        || !clamp_eligible(&r.desc, r.host.placed.load(Relaxed), r.host.state)
    {
        return 0;
    }
    let target = match mode {
        MipClampMode::MaxDim(dim) => Some(dim),
        _ => {
            let video = crate::host::host().and_then(|h| h.video_memory());
            let extra = video.map_or(0, |[budget, ..]| super::targets_room_kept(budget))
                .saturating_add(crate::shadow::card_room_kept());
            let video = video.map(|[budget, used, shared, shared_used]| {
                [budget, used.saturating_sub(crate::pool::texture_room()), shared, shared_used]
            });
            let filled = KINDS[1].load(Relaxed).saturating_add(crate::tiles::given_total());
            auto_mip_clamp_target_dim(mode, video, extra, filled, budget_override_bytes())
        }
    };
    let base = target.map_or(0, |dim| compute_mip_clamp_base(&r.desc, dim));
    if base != 0 && alias_of(this).is_none() { base } else { 0 }
}

/// `[alive_clamped, alive_saved_bytes, total_clamped_made, total_saved_bytes]`.
pub(crate) static MIP_CLAMP_STATS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

pub(super) fn record_allocation(texture: &ID3D12Resource, baseline: &ResourceDesc, bytes: u64) -> u64 {
    let full_bytes = crate::host::host().map_or(bytes, |host| {
        // SAFETY: a live resource and device. Use the host's format and flags.
        unsafe {
            let mut d = texture.GetDesc();
            d.Alignment = 0;
            d.Width = baseline.width;
            d.Height = baseline.height;
            d.MipLevels = baseline.mip_levels;
            host.device.GetResourceAllocationInfo(0, &[d]).SizeInBytes
        }
    });
    let saved = if full_bytes == u64::MAX { 0 } else { full_bytes.saturating_sub(bytes) };
    MIP_CLAMP_STATS[0].fetch_add(1, Relaxed);
    MIP_CLAMP_STATS[1].fetch_add(saved, Relaxed);
    MIP_CLAMP_STATS[2].fetch_add(1, Relaxed);
    MIP_CLAMP_STATS[3].fetch_add(saved, Relaxed);
    saved
}

pub(super) fn record_release(saved: u64) {
    MIP_CLAMP_STATS[0].fetch_sub(1, Relaxed);
    MIP_CLAMP_STATS[1].fetch_sub(saved, Relaxed);
}

/// Keeps both the resource identity and the displaced copy alive until rebinding.
struct PendingUnclamp {
    resource: usize,
    previous: Option<Made>,
}

impl Drop for PendingUnclamp {
    fn drop(&mut self) {
        resource_release(self.resource as This);
    }
}

static UNCLAMPED_REBINDS: Mutex<Vec<PendingUnclamp>> = Mutex::new(Vec::new());
static HAS_UNCLAMPED_REBINDS: AtomicBool = AtomicBool::new(false);

/// Explicit operations name original subresource numbers. Disable both floors
/// before looking up their resources, and keep any recorded uses of the old copy alive.
pub(crate) fn ensure_unclamped(this: This) {
    if !enabled() {
        super::keep_whole(this);
        return;
    }
    if this.is_null() {
        return;
    }
    let r = resource(this);
    if !fills(&r.desc) || r.host.whole.load(Relaxed) {
        return;
    }
    let staged = {
        let mut made = r.host.made.lock().unwrap_or_else(|e| e.into_inner());
        if r.host.whole.swap(true, Relaxed) {
            return;
        }
        if crate::tiles::max_size().is_some() {
            CAP_LIFTED.fetch_add(1, Relaxed);
        }
        let (previous, staged) = if let Some((host, _)) = made.as_ref().filter(|(host, _)| host.base != 0) {
            host.replaced.store(true, Relaxed);
            r.host.capped.store(false, Relaxed);
            let staged = crate::texture::take_staged(this);
            r.host.grown.store(true, Relaxed);
            (made.take(), staged)
        } else if r.host.grown.load(Relaxed)
            || made.as_ref().is_some_and(|(host, _)| host.grown.load(Relaxed))
        {
            // A view already requested a wider copy. The mirrored slot can
            // still show the earlier one until the upload completes.
            (None, None)
        } else {
            return;
        };
        let mut queue = UNCLAMPED_REBINDS.lock().unwrap_or_else(|e| e.into_inner());
        add_ref(this);
        queue.push(PendingUnclamp { resource: this as usize, previous });
        HAS_UNCLAMPED_REBINDS.store(true, Release);
        staged
    };
    drop(staged);
}

pub(crate) fn has_unclamped_rebinds() -> bool {
    HAS_UNCLAMPED_REBINDS.load(Acquire)
}

/// Under the submission lock, after heap sync. Settle submitted readers before
/// rewriting the mirrored descriptors. Dynamic-table snapshots are not rewritten:
/// recorded but unsubmitted lists may still reference the displaced copy through
/// them. Snapshots keep their own references; the displaced copy itself goes
/// through fence-aware `crate::host::retire`.
pub(crate) fn flush_unclamped_rebinds(settle: impl FnOnce()) {
    if !has_unclamped_rebinds() {
        return;
    }
    let pending = {
        let mut queue = UNCLAMPED_REBINDS.lock().unwrap_or_else(|e| e.into_inner());
        HAS_UNCLAMPED_REBINDS.store(false, Release);
        std::mem::take(&mut *queue)
    };
    if pending.is_empty() {
        return;
    }
    settle();
    let resources: HashSet<_> = pending.iter().map(|item| item.resource).collect();
    let placed = pending
        .iter()
        .filter_map(|item| {
            let r = resource(item.resource as This);
            r.host.placed.load(Relaxed).then(|| r.data() as u64)
        })
        .collect();
    crate::descriptor::host::rebind_unclamped_srvs(&resources, &placed);
    for mut item in pending {
        if let Some(previous) = item.previous.take() {
            crate::host::retire(previous);
        }
    }
}

#[cfg(test)]
#[path = "texture_mip_clamp_tests.rs"]
mod tests;
