//! Reclaim idle shadow reference snapshots and obsolete loose/exact copies.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, RwLock, atomic::Ordering::Relaxed};

use super::{BUFFERS, EXACT, KEPT_BYTES, LET_GO, LOOSE, Shadow, buffers_read, copy_bytes, keeps, let_go_tally};

// START_MODULE_CONTRACT
// PURPOSE: Lower private commit when a reference is cold or title memory was remapped.
// SCOPE: No GPU work in flight may lose backing or previously captured bytes.
// DEPENDS: Shadow's pending/retained coverage state; private ram_trim backing; host retirement.
// LINKS: M-SHADOW-SNAPSHOT-RECLAIM, R-SHADOW-RAM-TRIM
// END_MODULE_CONTRACT
// START_MODULE_MAP
// sweep: visit retained references, once per shadow, after normal window retirement.
// reclaim: lock flight state and backing before releasing commit.
// remapped: retire unused loose/exact copies and discard dead pieces in remaining copies.
// END_MODULE_MAP

// START_BLOCK_REFERENCE
pub(super) fn sweep(cold_ms: u64, critical: bool, now: u64) {
    if !keeps() {
        return;
    }
    let mut live: Vec<_> = buffers_read().values().filter_map(|e| e.shadow.clone()).collect();
    live.extend(LOOSE.read().unwrap_or_else(|e| e.into_inner()).values().cloned());
    live.extend(EXACT.read().unwrap_or_else(|e| e.into_inner()).values().map(|(s, _)| s.clone()));
    let mut seen = HashSet::new();
    for shadow in live {
        if seen.insert(shadow.id) {
            reclaim(&shadow, Reclaim::Sweep { cold_ms, critical, now });
        }
    }
}

enum Reclaim {
    Sweep { cold_ms: u64, critical: bool, now: u64 },
    Remap { at: u64, end: u64 },
}

// START_CONTRACT: reclaim
// PURPOSE: Reclaim exclusively owned snapshot storage without discarding any flight's bytes.
// INPUTS: One shadow, and a sweep policy or the interval that no longer holds the old title bytes.
// OUTPUTS: Committed bytes the VM actually released.
// SIDE_EFFECTS: Holds pending -> before -> kept; invalidates retained coverage for discarded bytes.
// END_CONTRACT
fn reclaim(shadow: &Shadow, reason: Reclaim) -> usize {
    if shadow.place.is_some() {
        return 0;
    }
    // `writing` also holds pending before before; do not wait on a renderer
    // from a title memory callback. A detached worker pin also blocks discard.
    let Ok(pending) = shadow.pending.try_lock() else { return 0 };
    if !pending.is_empty() {
        return 0;
    }
    let Ok(mut before) = shadow.before.try_lock() else { return 0 };
    let Some(snapshot) = before.as_mut() else { return 0 };
    let settled_ms = snapshot.settled_ms();
    let Some(backing) = snapshot.exclusive_backing() else { return 0 };
    let Ok(mut kept) = shadow.kept.try_lock() else { return 0 };
    match reason {
        Reclaim::Sweep { cold_ms, critical, now } => {
            if !backing.is_large() {
                return 0;
            }
            if critical || now.saturating_sub(settled_ms) >= cold_ms {
                // No flight can read these bytes. Invalidate cached coverage so
                // the next capture reads host data, not freshly committed zeros.
                kept.0.clear();
                if std::mem::take(&mut kept.1) {
                    KEPT_BYTES.fetch_sub(shadow.len, Relaxed);
                }
                backing.discard_all()
            } else {
                // These bytes have the same value after recommit; keep coverage.
                backing.decommit_zero_pieces()
            }
        }
        Reclaim::Remap { at, end } => {
            let from = at.saturating_sub(shadow.start) as usize;
            let to = end.saturating_sub(shadow.start).min(shadow.len) as usize;
            if from >= to {
                return 0;
            }
            if !kept.0.is_empty() {
                let pieces = shadow.pieces(from, to);
                let len = kept.0.len();
                kept.0[pieces.start.min(len)..pieces.end.min(len)].fill(false);
            }
            if backing.is_large() { backing.discard_byte_range(from, to) } else { 0 }
        }
    }
}
// END_BLOCK_REFERENCE

// START_BLOCK_REMAPPED
pub(super) fn remapped(at: u64, len: u64) {
    let end = at.saturating_add(len);
    if at >= end {
        return;
    }
    let mut affected = Vec::new();
    let mut gone = Vec::new();
    remap_windows(&LOOSE, |s| s, at, end, &mut affected, &mut gone);
    remap_windows(&EXACT, |(s, _)| s, at, end, &mut affected, &mut gone);
    if let Ok(buffers) = BUFFERS.try_read() {
        affected.extend(
            buffers.range(..(end, 0))
                .filter(|(_, entry)| entry.end > at)
                .filter_map(|(_, entry)| entry.shadow.clone()),
        );
    }
    let mut seen = HashSet::new();
    for shadow in affected {
        if seen.insert(shadow.id) {
            reclaim(&shadow, Reclaim::Remap { at, end });
        }
    }
    let tally = let_go_tally(gone.iter().map(|s| (s.len, copy_bytes(s))));
    for (counter, value) in LET_GO.iter().zip(tally) {
        counter.fetch_add(value, Relaxed);
    }
    for shadow in gone {
        crate::host::retire(shadow);
    }
}

fn remap_windows<V>(
    table: &RwLock<BTreeMap<(u64, u64), V>>,
    shadow_of: impl Fn(&V) -> &Arc<Shadow>,
    at: u64,
    end: u64,
    affected: &mut Vec<Arc<Shadow>>,
    gone: &mut Vec<Arc<Shadow>>,
) {
    // An in-place/imported allocation can hold LOOSE while it asks about
    // title mappings. Never wait for a shadow index from remap notification.
    let Ok(mut table) = table.try_write() else { return };
    let mut remove = Vec::new();
    for (&key, entry) in table.range(..(end, 0)) {
        let shadow = shadow_of(entry);
        let to = shadow.start.saturating_add(shadow.len);
        if shadow.place.is_some() || to <= at {
            continue;
        }
        if at <= shadow.start && to <= end
            && Arc::strong_count(shadow) == 1
            && shadow.pending.try_lock().is_ok_and(|ranges| ranges.is_empty())
            && crate::host::references(&shadow.buffer) == 1
        {
            remove.push(key);
        } else {
            affected.push(shadow.clone());
        }
    }
    for key in remove {
        if let Some(entry) = table.remove(&key) {
            gone.push(shadow_of(&entry).clone());
        }
    }
}
// END_BLOCK_REMAPPED

#[cfg(test)]
#[path = "snapshot_reclaim_ram_trim_tests.rs"]
mod ram_trim_tests;
