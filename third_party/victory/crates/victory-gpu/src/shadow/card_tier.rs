//! Opt-in admission headroom and fence-checked eviction for small local buffers.

use super::card_budget::{Budget, LARGE, large_reserve, opt_in, pressure_target};
use super::*;
use crate::host::Host;
use windows::Win32::Graphics::Direct3D12::ID3D12Fence;

const BATCH: u64 = 16 << 20;

pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| opt_in(std::env::var("VICTORY_CARD_DEMOTE").ok().as_deref()))
}

fn pressure_pct() -> u64 {
    static PCT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *PCT.get_or_init(|| {
        std::env::var("VICTORY_CARD_PRESSURE_PCT")
            .ok()
            .as_deref()
            .and_then(parse_mib)
            .map_or(88, |pct| pct.clamp(50, 98))
    })
}

fn reserve() -> u64 {
    static RESERVE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *RESERVE.get_or_init(|| {
        let (least, most) = card_limits();
        large_reserve(
            least,
            most,
            std::env::var("VICTORY_CARD_LARGE_RESERVE_MB")
                .ok()
                .as_deref()
                .and_then(parse_mib),
        )
    })
}

static BUDGET: Mutex<Budget> = Mutex::new(Budget::new());

pub(super) fn room_taken() -> u64 {
    BUDGET
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .unreserved()
}

pub(super) fn release(size: u64, pressure: bool) {
    BUDGET
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .release(size, pressure);
}

/// Demotions, bytes demoted, and small admissions refused for large headroom.
pub(crate) static STATS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];

pub(super) enum Placement {
    Local(ID3D12Resource),
    System,
    Reclaim(u64),
}

/// Runs under the buffer table's write lock, so no other admission can raise
/// the active count between the check and successful allocation. Drops can
/// still lower it. No credit is consumed on allocation failure.
pub(super) fn place(host: &Host, size: u64, retry: bool) -> Placement {
    let (least, most) = card_limits();
    if !cards() || size < least || size > most || size >= 1 << 30 || !host.card_ready() {
        return Placement::System;
    }
    let memory = host.video_memory();
    let base_pct = if card_room_kept_on() { 97 } else { 80 };
    let small_pct = base_pct.min(85).min(pressure_pct());
    let limit = if size < LARGE { small_pct } else { base_pct };
    let reserve = reserve();
    let budget = BUDGET.lock().unwrap_or_else(|e| e.into_inner());
    if budget.reserved_for_large(size, most, reserve) {
        STATS[2].fetch_add(1, Ordering::Relaxed);
        return Placement::System;
    }
    let (shortfall, dxgi) = budget.shortfall(size, most, memory, limit);
    if shortfall != 0 {
        if retry && size >= LARGE && shortfall <= budget.small {
            return Placement::Reclaim(shortfall);
        }
        if size >= LARGE && dxgi != 0 {
            CARD_REFUSED[0].fetch_add(1, Ordering::Relaxed);
            CARD_REFUSED[1].fetch_add(size, Ordering::Relaxed);
        }
        return Placement::System;
    }
    drop(budget);
    let Some(buffer) = host.uav_buffer(size) else {
        return Placement::System;
    };
    BUDGET.lock().unwrap_or_else(|e| e.into_inner()).admit(size);
    CARD_BYTES[0].fetch_add(size, Ordering::Relaxed);
    Placement::Local(buffer)
}

/// The queue domains whose values cannot be compared with the main fence.
#[derive(Clone, Copy)]
pub(crate) enum Queue {
    Direct,
    RelayCopy,
    Upload,
}

/// The direct queue may read any buffer exposed via a bindless descriptor.
/// The bound slot holds a resource reference; this covers in-flight uses
/// even after a descriptor is removed, without scanning the heap per submit.
static DESCRIPTORS_FENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn note_descriptors_fence(value: u64) {
    DESCRIPTORS_FENCE.fetch_max(value, Ordering::Release);
}

#[derive(Default)]
pub(super) struct Usage {
    pub(super) last_fence: AtomicU64,
    pub(super) descriptor: AtomicBool,
    pub(super) gpu_written: AtomicBool,
    access: AtomicU64,
    auxiliary: Mutex<[Option<(ID3D12Fence, u64)>; 3]>,
}

impl Usage {
    pub(super) fn access(&self) {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        self.access
            .fetch_max(NEXT.fetch_add(1, Ordering::Relaxed), Ordering::Relaxed);
    }

    pub(super) fn stamp(&self, queue: Queue, fence: &ID3D12Fence, value: u64) {
        self.auxiliary.lock().unwrap_or_else(|e| e.into_inner())[queue as usize] =
            Some((fence.clone(), value));
    }

    fn main_stamp(&self) -> u64 {
        let last = self.last_fence.load(Ordering::Acquire);
        if self.descriptor.load(Ordering::Acquire) {
            last.max(DESCRIPTORS_FENCE.load(Ordering::Acquire))
        } else {
            last
        }
    }

    fn idle(&self, completed: u64) -> bool {
        completed != u64::MAX
            && self.main_stamp() <= completed
            && self
                .auxiliary
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .flatten()
                .all(|(fence, value)| {
                    // SAFETY: the stamp holds a live fence for this queue domain.
                    let done = unsafe { fence.GetCompletedValue() };
                    done != u64::MAX && done >= *value
                })
    }
}

fn eligible(shadow: &Arc<Shadow>, completed: u64) -> Option<(&Card, &Usage)> {
    let card = shadow.card.as_ref()?;
    let usage = shadow.card_usage.as_deref()?;
    if !card.tier_local
        || card.size >= LARGE
        || Arc::strong_count(shadow) != 1
        || shadow.has_pending.load(Ordering::Acquire)
        || !usage.idle(completed)
        || crate::host::references(&shadow.buffer) != 1
    {
        return None;
    }
    Some((card, usage))
}

fn needs_readback(card: &Card, usage: &Usage) -> bool {
    if usage.last_fence.load(Ordering::Acquire) == 0 && !usage.gpu_written.load(Ordering::Acquire) {
        return false;
    }
    let dirty = card.dirty.lock().unwrap_or_else(|e| e.into_inner());
    !outside(0, card.size as usize, dirty.clone()).is_empty()
}

#[derive(Clone, Copy)]
struct Candidate {
    key: (u64, usize),
    id: u64,
    main_stamp: u64,
    size: u64,
    access: u64,
    read: bool,
}

impl Candidate {
    fn matches(self, usage: &Usage) -> bool {
        usage.main_stamp() == self.main_stamp && usage.access.load(Ordering::Acquire) == self.access
    }
}

/// Replace the backing only while the map owns the only reference and the
/// main fence has not changed since selection. List/root references use Arc;
/// descriptor slots also have to have let go of the COM resource.
fn replace(
    entry: &mut Entry,
    candidate: Candidate,
    completed: u64,
    bytes: Option<&[u8]>,
    pressure: bool,
    resources: &mut Vec<ID3D12Resource>,
) -> u64 {
    let Some(shadow) = entry.shadow.as_ref() else {
        return 0;
    };
    let Some((card, usage)) = eligible(shadow, completed) else {
        return 0;
    };
    if shadow.id != candidate.id
        || !candidate.matches(usage)
        || bytes.map_or_else(
            || needs_readback(card, usage),
            |b| b.len() != card.size as usize,
        )
    {
        return 0;
    }
    // A weak lookup may have acquired an Arc after the eligibility check.
    let mut shadow = match Arc::try_unwrap(entry.shadow.take().unwrap()) {
        Ok(shadow) => shadow,
        Err(shared) => {
            entry.shadow = Some(shared);
            return 0;
        }
    };
    // Recheck after gaining unique ownership, covering any weak upgrade
    // between the first check and try_unwrap. Re-register an unchanged copy
    // if that upgrade used it and has already let go.
    if shadow.has_pending.load(Ordering::Acquire)
        || !shadow.card_usage.as_ref().is_some_and(|usage| {
            candidate.matches(usage)
                && usage.idle(completed)
                && (bytes.is_some() || !needs_readback(shadow.card.as_ref().unwrap(), usage))
        })
        || crate::host::references(&shadow.buffer) != 1
    {
        let shadow = Arc::new(shadow);
        CARDED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Arc::downgrade(&shadow));
        entry.shadow = Some(shadow);
        return 0;
    }
    let card = shadow.card.take().unwrap();
    shadow.card_usage = None;
    if let Some(bytes) = bytes {
        let dirty = card.dirty.into_inner().unwrap_or_else(|e| e.into_inner());
        for (a, b) in outside(0, bytes.len(), dirty) {
            // SAFETY: unique shadow, live mapping and an exact-sized readback.
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr().add(a), shadow.data.add(a), b - a)
            };
        }
    }
    resources.push(std::mem::replace(&mut shadow.buffer, card.staging));
    if let Some(before) = card.before.into_inner().unwrap_or_else(|e| e.into_inner()) {
        resources.push(before);
        CARD_BYTES[1].fetch_sub(card_hash_bytes(card.size), Ordering::Relaxed);
    }
    // SAFETY: the old staging buffer is the new, live shader backing.
    shadow.gpu = unsafe { shadow.buffer.GetGPUVirtualAddress() };
    shadow.width = Default::default();
    shadow.id = next_shadow_id();
    entry.shadow = Some(Arc::new(shadow));
    release(card.size, pressure);
    CARD_BYTES[0].fetch_sub(card.size, Ordering::Relaxed);
    STATS[0].fetch_add(1, Ordering::Relaxed);
    STATS[1].fetch_add(card.size, Ordering::Relaxed);
    card.size
}

/// At most one bounded readback per attempt. Never wait with the table or
/// budget locked; retry the placement only after releasing the old buffers.
pub(super) fn reclaim(host: &Host, target: u64, pressure: bool) -> u64 {
    static RECLAIM: Mutex<()> = Mutex::new(());
    let Ok(_reclaim) = RECLAIM.try_lock() else {
        return 0;
    };
    let mut freed = 0;
    let mut resources = Vec::new();
    let (stamps, sources) = {
        let mut map = buffers();
        let completed = host.completed_fence();
        let mut candidates: Vec<_> = map
            .iter()
            .filter_map(|(&key, entry)| {
                let shadow = entry.shadow.as_ref()?;
                let (card, usage) = eligible(shadow, completed)?;
                Some(Candidate {
                    key,
                    id: shadow.id,
                    main_stamp: usage.main_stamp(),
                    size: card.size,
                    access: usage.access.load(Ordering::Relaxed),
                    read: needs_readback(card, usage),
                })
            })
            .collect();
        candidates.sort_unstable_by_key(|c| (c.access, c.key));
        let mut stamps = Vec::new();
        let mut sources = Vec::new();
        let mut planned = 0;
        for c in candidates {
            if freed + planned >= target {
                break;
            }
            let entry = map.get_mut(&c.key).unwrap();
            if !c.read {
                freed += replace(entry, c, completed, None, pressure, &mut resources);
            } else if planned + c.size <= BATCH {
                sources.push((entry.shadow.as_ref().unwrap().clone(), c.size));
                stamps.push(c);
                planned += c.size;
            }
        }
        (stamps, sources)
    };
    resources.clear();
    if !sources.is_empty()
        && let Some(readback) = host.demote_readback(sources)
    {
        let mut map = buffers();
        let completed = host.completed_fence();
        for (c, bytes) in stamps.into_iter().zip(readback) {
            if let Some(entry) = map.get_mut(&c.key) {
                freed += replace(entry, c, completed, Some(&bytes), pressure, &mut resources);
            }
        }
    }
    freed
}

pub(crate) fn under_pressure(host: &Host, memory: Option<[u64; 4]>) {
    if !cards() {
        return;
    }
    let small = BUDGET.lock().unwrap_or_else(|e| e.into_inner()).small;
    let target = pressure_target(memory, small, pressure_pct());
    if target > 0 {
        reclaim(host, target, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readback_preserves_gpu_and_cpu_dirty_bytes() {
        const BYTES: u64 = 64 << 10;
        let host = host().expect("D3D12 is needed for the readback test");
        let local = host.uav_buffer(BYTES).unwrap();
        let (staging, data) = host.shared_buffer(BYTES).unwrap();
        let (upload, upload_data) = host.upload_buffer(BYTES).unwrap();
        let commands = host.new_list().unwrap();
        // SAFETY: these fresh allocations are live and large enough; no GPU
        // work uses them yet, and the list is open.
        unsafe {
            std::ptr::write_bytes(upload_data, 0x34, BYTES as usize);
            std::ptr::write_bytes(data, 0x72, BYTES as usize);
            commands.list.CopyBufferRegion(&local, 0, &upload, 0, BYTES);
        }
        commands.close();
        let fence = host.execute(&[&commands]);
        let mut shadow = Shadow::new(local, data, 0, 0, BYTES);
        shadow.card_usage = Some(Box::default());
        shadow.note_gpu_fence(fence);
        shadow.card = Some(Card {
            staging,
            before: Mutex::default(),
            dirty: Mutex::new(vec![(5, 11)]),
            has_dirty: AtomicBool::new(true),
            sent: Mutex::default(),
            has_sent: AtomicBool::new(false),
            size: BYTES,
            tier_local: true,
        });
        BUDGET
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .admit(BYTES);
        CARD_BYTES[0].fetch_add(BYTES, Ordering::Relaxed);
        let shadow = Arc::new(shadow);
        assert!(eligible(&shadow, fence - 1).is_none());
        let shared = shadow.clone();
        assert!(eligible(&shadow, fence).is_none());
        drop(shared);
        let descriptor = shadow.buffer.clone();
        assert!(eligible(&shadow, fence).is_none());
        drop(descriptor);
        let (card, usage) = eligible(&shadow, fence).expect("idle, uniquely held copy");
        assert!(needs_readback(card, usage));
        let cand = Candidate {
            key: (0, 0),
            id: shadow.id(),
            main_stamp: usage.main_stamp(),
            size: BYTES,
            access: 0,
            read: true,
        };
        let result = host
            .demote_readback(vec![(shadow.clone(), BYTES)])
            .expect("card buffer readback");
        let mut entry = Entry {
            end: BYTES,
            shadow: Some(shadow),
        };
        let mut resources = Vec::new();
        assert_eq!(
            replace(
                &mut entry,
                cand,
                fence,
                Some(&result[0]),
                false,
                &mut resources
            ),
            BYTES
        );
        let demoted = entry.shadow.unwrap();
        assert!(!demoted.carded());
        assert_ne!(demoted.id(), cand.id);
        // SAFETY: the demoted backing still owns its mapped staging area.
        let got = unsafe { std::slice::from_raw_parts(demoted.data, BYTES as usize) };
        assert!(got[..5].iter().all(|&b| b == 0x34));
        assert_eq!(&got[5..11], &[0x72; 6]);
        assert!(got[11..].iter().all(|&b| b == 0x34));
    }
}
