//! Idle backing for RAM-trim shadow snapshots. The legacy pool is unchanged.

use std::sync::Mutex;

use crate::ram_trim::{SnapshotBuffer, TrimPolicy, TrimThresholds, note_pool_trimmed};

// START_MODULE_CONTRACT
// PURPOSE: Bound idle snapshot commit and retain a limited number of VM addresses for reuse.
// SCOPE: Unowned snapshot backing only; no title memory, D3D mappings, or flight state.
// DEPENDS: ram_trim storage and thresholds, host clock, legacy pool fit and slot limits.
// LINKS: M-SHADOW-SNAPSHOT-POOL, R-SHADOW-RAM-TRIM
// END_MODULE_CONTRACT
// START_MODULE_MAP
// Pool::take: reuse the smallest matching reservation without eagerly recommitting it.
// Pool::trim: retain warm entries and discard dead contents from cold reservations.
// recycle/sweep/flush: apply the active policy or release backing for a commit retry.
// END_MODULE_MAP

// START_BLOCK_POOL
const WARM_CAP: usize = 512 << 20;

struct Entry {
    backing: SnapshotBuffer,
    returned_ms: u64,
}

#[derive(Default)]
struct Pool {
    /// Descending logical length, so a fit can be reused without resizing it.
    entries: Vec<Entry>,
    committed: usize,
    large: usize,
    used: u64,
}

struct Limits {
    warm_bytes: usize,
    slots: usize,
    large_slots: usize,
    idle_ms: u64,
}

impl Limits {
    fn from_policy(policy: &TrimThresholds, pool_idle: bool) -> Self {
        let slots = super::snapshots_kept();
        if policy.critical_memory || pool_idle {
            return Self { warm_bytes: 0, slots: 0, large_slots: 0, idle_ms: policy.pool_idle_ms };
        }
        Self {
            warm_bytes: (policy.shadow_pool_warm_bytes as usize).min(WARM_CAP),
            slots,
            large_slots: policy.shadow_pool_reserve_slots.min(slots),
            idle_ms: policy.pool_idle_ms,
        }
    }
}

impl Pool {
    fn take(&mut self, len: usize) -> Option<SnapshotBuffer> {
        let i = super::snapshot_fit(&self.entries, len, |entry| entry.backing.len())?;
        let entry = self.entries.remove(i);
        self.committed -= entry.backing.committed_bytes();
        self.large -= usize::from(entry.backing.is_large());
        Some(entry.backing)
    }

    fn insert(&mut self, backing: SnapshotBuffer, returned_ms: u64) {
        let i = self.entries.partition_point(|entry| entry.backing.len() >= backing.len());
        self.committed += backing.committed_bytes();
        self.large += usize::from(backing.is_large());
        self.entries.insert(i, Entry { backing, returned_ms });
    }

    fn over(&self, limits: &Limits) -> bool {
        self.committed > limits.warm_bytes || self.entries.len() > limits.slots || self.large > limits.large_slots
    }

    fn trim(&mut self, limits: &Limits, now: u64) -> Vec<Entry> {
        if limits.slots == 0 {
            return self.drain();
        }
        if !self.over(limits)
            && self.entries.iter().all(|entry| now.saturating_sub(entry.returned_ms) < limits.idle_ms)
        {
            return Vec::new();
        }
        // Prefer recent reuse. Trim only the backing retained as a reservation;
        // excess entries are dropped outright instead of decommitting twice.
        self.entries.sort_by_key(|entry| {
            (std::cmp::Reverse(entry.returned_ms), entry.backing.len())
        });
        let (mut slots, mut large, mut committed) = (0, 0, 0);
        let dropped = self.entries.extract_if(.., |entry| {
            let is_large = entry.backing.is_large();
            if slots == limits.slots || (is_large && large == limits.large_slots) {
                return true;
            }
            let bytes = entry.backing.committed_bytes();
            let hot = now.saturating_sub(entry.returned_ms) < limits.idle_ms;
            if hot && bytes <= limits.warm_bytes - committed {
                committed += bytes;
            } else if is_large {
                entry.backing.discard_all();
                if entry.backing.committed_bytes() != 0 {
                    // A refused decommit must not let us exceed the commit cap.
                    return true;
                }
            } else {
                return true;
            }
            large += usize::from(is_large);
            slots += 1;
            false
        }).collect();
        self.entries.sort_by_key(|entry| std::cmp::Reverse(entry.backing.len()));
        self.large = large;
        self.committed = committed;
        dropped
    }

    fn drain(&mut self) -> Vec<Entry> {
        self.committed = 0;
        self.large = 0;
        std::mem::take(&mut self.entries)
    }
}
// END_BLOCK_POOL

// START_BLOCK_SHARED_POOL
static POOL: Mutex<Pool> = Mutex::new(Pool { entries: Vec::new(), committed: 0, large: 0, used: 0 });

pub(super) fn take(len: usize) -> Option<SnapshotBuffer> {
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    pool.used = crate::host::moment();
    pool.take(len)
}

pub(super) fn recycle(backing: SnapshotBuffer) {
    let policy = TrimPolicy::cached();
    let Some(policy) = policy.thresholds() else { return };
    let limits = Limits::from_policy(policy, false);
    let now = crate::host::now_ms();
    reclaim(|pool| {
        pool.used = crate::host::moment();
        pool.insert(backing, now);
        let before = pool.committed;
        let dropped = if pool.over(&limits) { pool.trim(&limits, now) } else { Vec::new() };
        (dropped, before)
    });
}

pub(super) fn sweep(policy: &TrimThresholds, now: u64) {
    let idle_ms = crate::host::ram_idle_ms(crate::host::available_physical_ram());
    reclaim(|pool| {
        let pool_idle = crate::host::long_ago(pool.used, idle_ms, 4);
        let before = pool.committed;
        (pool.trim(&Limits::from_policy(policy, pool_idle), now), before)
    });
}

/// During a recommit failure the only memory we can release here is idle backing.
pub(super) fn flush() {
    reclaim(|pool| {
        let before = pool.committed;
        (pool.drain(), before)
    });
}

fn reclaim(f: impl FnOnce(&mut Pool) -> (Vec<Entry>, usize)) {
    let (dropped, trimmed) = {
        let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
        let (dropped, before) = f(&mut pool);
        (dropped, before - pool.committed)
    };
    // Free the discarded allocations outside the pool lock.
    drop(dropped);
    if trimmed != 0 {
        note_pool_trimmed(trimmed as u64);
    }
}

#[cfg(feature = "diagnostics")]
pub(super) fn memory() -> (u64, u64) {
    let pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    (pool.entries.len() as u64, pool.committed as u64)
}
// END_BLOCK_SHARED_POOL

#[cfg(test)]
#[path = "snapshot_pool_ram_trim_tests.rs"]
mod ram_trim_tests;
