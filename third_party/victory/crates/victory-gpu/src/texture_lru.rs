//! Pressure recovery for reloadable host textures. Idle submits may opt in
//! to yielding superseded views or unmaking cold shader-heap aliases.

use super::{KINDS, Resource, ResourceDesc, card_use, footprint, room_percent, texture_floor_bytes, textures};
use crate::{descriptor, frame, host, object::This, texture_heap_evict, tiles};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

#[path = "texture_evict_scan.rs"]
pub(crate) mod scan;

const ROOM_TARGET: u64 = 32 << 20;
const SUBMIT_TARGET: u64 = 64 << 20;
static LAST_SCAN: Mutex<Option<(u64, u64)>> = Mutex::new(None);
static PENDING: Mutex<Option<Request>> = Mutex::new(None);
static STATS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];

pub(crate) fn enabled() -> bool {
    #[cfg(test)]
    if let Some(enabled) = TEST_EVICTION.get() {
        return enabled;
    }
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| crate::switch::opt_in(std::env::var("VICTORY_TEXTURE_LRU_EVICT").ok().as_deref()))
}

#[derive(Clone, Copy)]
struct Budget {
    filled_bytes: u64,
    texture_limit_bytes: Option<u64>,
    local_budget_bytes: Option<u64>,
    local_used_bytes: Option<u64>,
    accounted_bytes: u64,
    pressure_limit_bytes: u64,
    has_room: bool,
}

impl Budget {
    fn read(memory: Option<[u64; 4]>) -> Self {
        static LIMIT: OnceLock<Option<u64>> = OnceLock::new();
        let texture_limit_bytes = *LIMIT.get_or_init(|| {
            let mb = std::env::var("VICTORY_TEXTURE_BUDGET_MB").ok()?;
            mb.trim().parse::<u64>().ok()?.checked_mul(1 << 20)
        });
        #[cfg(test)]
        let texture_limit_bytes = TEST_LIMIT.get().or(texture_limit_bytes);
        let filled_bytes = KINDS[1].load(Relaxed) + tiles::given_total();
        let local_budget_bytes = memory.map(|m| m[0]);
        let (accounted_bytes, pressure_limit_bytes) = match (texture_limit_bytes, memory) {
            (Some(limit), _) => (filled_bytes, limit),
            (None, _) if filled_bytes < texture_floor_bytes(local_budget_bytes) => {
                (filled_bytes, texture_floor_bytes(local_budget_bytes))
            }
            (None, Some([budget, used, ..])) => (card_use(budget, used), budget / 100 * room_percent()),
            (None, None) => (filled_bytes, 4 << 30),
        };
        Self {
            filled_bytes,
            texture_limit_bytes,
            local_budget_bytes,
            local_used_bytes: memory.map(|m| m[1]),
            accounted_bytes,
            pressure_limit_bytes,
            has_room: accounted_bytes < pressure_limit_bytes,
        }
    }
}

#[cfg(test)]
thread_local! {
    static TEST_EVICTION: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    static TEST_LIMIT: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// Read-only and safe under a descriptor slot lock.
pub(crate) fn has_room_lately() -> bool {
    static LAST: AtomicU64 = AtomicU64::new(0);
    #[cfg(test)]
    if TEST_LIMIT.get().is_some() {
        return Budget::read(None).has_room;
    }
    let tenth = host::now_ms() / 100 + 1;
    let cached = LAST.load(Relaxed);
    if cached >> 1 == tenth {
        return cached & 1 != 0;
    }
    let has_room = Budget::read(host::host().and_then(host::Host::video_memory)).has_room;
    LAST.store((tenth << 1) | u64::from(has_room), Relaxed);
    has_room
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TextureInfo {
    address: usize,
    width: u64,
    height: u32,
    depth_or_layers: u16,
    format: u32,
    bytes: Option<u64>,
}

impl TextureInfo {
    fn new(address: usize, desc: &ResourceDesc, bytes: Option<u64>) -> Self {
        Self {
            address,
            width: desc.width,
            height: desc.height,
            depth_or_layers: desc.depth_or_array_size,
            format: desc.format,
            bytes,
        }
    }
}

#[derive(Clone, Copy)]
struct Request {
    bytes: u64,
    texture: Option<TextureInfo>,
}

fn request_later(request: Request) {
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(old) = pending.as_mut() {
        if request.texture.is_some() && (old.texture.is_none() || request.bytes >= old.bytes) {
            old.texture = request.texture;
        }
        old.bytes = old.bytes.max(request.bytes);
    } else {
        *pending = Some(request);
    }
}

/// At most one opportunistic scan per frame and completed fence step.
fn recover(h: &host::Host, request: Request) -> EvictOutcome {
    let completed = h.completed_fence();
    let stamp = (frame::current(), completed);
    let mut last = LAST_SCAN.lock().unwrap_or_else(|e| e.into_inner());
    if last.is_some_and(|previous| previous >= stamp) {
        drop(last);
        if request.texture.is_some() {
            request_later(request);
        }
        return EvictOutcome::default();
    }
    *last = Some(stamp);
    drop(last);
    let outcome = evict_lru_textures(request.bytes, completed, EvictPolicy::default());
    if outcome.evicted_bytes < request.bytes {
        request_later(request);
    }
    outcome
}

pub(super) fn room(memory: Option<[u64; 4]>) -> bool {
    if Budget::read(memory).has_room {
        return true;
    }
    if let Some(h) = host::host() {
        let outcome = recover(h, Request { bytes: ROOM_TARGET, texture: None });
        return outcome.evicted_count != 0 && Budget::read(h.video_memory().or(memory)).has_room;
    }
    false
}

/// Called without the resource's host-copy lock. Retry allocation only if
/// the pass detached a copy; submit can yield pins that this path cannot.
pub(super) fn recover_allocation(info: TextureInfo) -> EvictOutcome {
    let Some(h) = host::host() else {
        return EvictOutcome::default();
    };
    recover(h, Request {
        bytes: info.bytes.unwrap_or(ROOM_TARGET).max(4 << 20),
        texture: Some(info),
    })
}

/// Service pending recovery under the submission lock, with heap sync joined.
pub(crate) fn evict_if_pressured(h: &host::Host) {
    let lru = enabled();
    if !lru && !kept_views_pending() {
        return;
    }
    let (started, finished) = host::submissions();
    let completed = h.completed_fence();
    if started != finished || completed == u64::MAX || completed < h.last_signalled_fence() {
        return;
    }
    let pending = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take();
    let before = Budget::read(h.video_memory());
    if before.has_room {
        return;
    }
    let yielded = descriptor::yield_kept_views();
    let target = pending.map_or(SUBMIT_TARGET, |request| request.bytes.max(SUBMIT_TARGET));
    let outcome = if lru {
        scan::evict(target, completed, EvictPolicy::default(), texture_heap_evict::enabled())
    } else {
        EvictOutcome::default()
    };
    let remaining = Budget::read(h.video_memory());
    if !remaining.has_room {
        let info = pending.and_then(|request| request.texture).or(outcome.texture).or_else(pressure_texture);
        warn_pressure("submit_pressure", info, outcome, remaining, yielded);
    }
}

/// Avoid querying the adapter on each submit and joining the heap worker early
/// on the common path. A failed pass may be tried again after the cooldown.
pub(crate) fn needs_submit_maintenance() -> bool {
    static LAST_PRESSURE: AtomicU64 = AtomicU64::new(0);
    if !enabled() && !kept_views_pending() {
        return false;
    }
    let tick = host::now_ms() / 100 + 1;
    if LAST_PRESSURE.load(Relaxed) == tick {
        return false;
    }
    let needed = PENDING.lock().unwrap_or_else(|e| e.into_inner()).is_some() || !has_room_lately();
    if needed {
        LAST_PRESSURE.store(tick, Relaxed);
    }
    needed
}

fn kept_views_pending() -> bool {
    descriptor::host::kept_views_yield() && descriptor::host::keeping_total() != 0
}

fn pressure_texture() -> Option<TextureInfo> {
    let all = textures();
    let at = *all.first()?;
    // SAFETY: the resource stays in the table until it is destroyed.
    let r = unsafe { &*(at as *const Resource) };
    Some(allocation_info(at as This, &r.desc))
}

pub(super) fn warn_refused(info: TextureInfo, outcome: EvictOutcome) {
    let remaining = Budget::read(host::host().and_then(host::Host::video_memory));
    warn_pressure("allocation_refused", Some(info), outcome, remaining, 0);
}

fn warn_pressure(
    reason: &str,
    info: Option<TextureInfo>,
    outcome: EvictOutcome,
    budget: Budget,
    yielded: usize,
) {
    tracing::warn!(
        reason,
        resource = ?info.map(|t| t.address),
        width = ?info.map(|t| t.width),
        height = ?info.map(|t| t.height),
        depth_or_layers = ?info.map(|t| t.depth_or_layers),
        format = ?info.map(|t| t.format),
        texture_bytes = ?info.and_then(|t| t.bytes),
        evicted_count = outcome.evicted_count,
        evicted_bytes = outcome.evicted_bytes,
        skipped_in_flight = outcome.skipped_in_flight,
        lru_total_count = STATS[0].load(Relaxed),
        lru_total_bytes = STATS[1].load(Relaxed),
        lru_total_skipped_in_flight = STATS[2].load(Relaxed),
        yielded_views = yielded,
        remaining_pressure = !budget.has_room,
        filled_bytes = budget.filled_bytes,
        texture_limit_bytes = ?budget.texture_limit_bytes,
        local_budget_bytes = ?budget.local_budget_bytes,
        local_used_bytes = ?budget.local_used_bytes,
        accounted_bytes = budget.accounted_bytes,
        pressure_limit_bytes = budget.pressure_limit_bytes,
        "texture pressure recovery did not suffice"
    );
}

#[derive(Clone, Copy)]
struct EvictPolicy {
    min_idle_ms: u64,
    min_idle_frames: u64,
    max_victims: usize,
}

impl Default for EvictPolicy {
    fn default() -> Self {
        Self {
            min_idle_ms: 1_000,
            min_idle_frames: 2,
            max_victims: 256,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct EvictOutcome {
    pub(super) evicted_count: u64,
    pub(super) evicted_bytes: u64,
    skipped_in_flight: u64,
    texture: Option<TextureInfo>,
}

/// Opportunistic LRU must never rewrite a shader heap, even with Tier 3 enabled.
fn evict_lru_textures(target: u64, completed: u64, policy: EvictPolicy) -> EvictOutcome {
    scan::evict(target, completed, policy, false)
}

pub(super) fn allocation_info(this: This, desc: &ResourceDesc) -> TextureInfo {
    TextureInfo::new(this as usize, desc, footprint(desc).map(|f| f.size))
}

#[cfg(test)]
#[path = "texture_lru_tests.rs"]
pub(crate) mod tests;
