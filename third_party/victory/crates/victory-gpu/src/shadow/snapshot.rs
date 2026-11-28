//! A CPU shadow's before-image, with opt-in VM backing.

use std::sync::{Arc, Mutex};

use crate::ram_trim::{SnapshotBuffer, enabled};

use super::{give_back, pooled, snapshot_pool};

// START_MODULE_CONTRACT
// PURPOSE: Check VM backing before any access; keep it alive across raw worker jobs.
// SCOPE: Private CPU snapshots. Legacy allocations keep the existing Vec behavior.
// DEPENDS: ram_trim storage, snapshot_pool, and the submission's range/lifetime rules.
// LINKS: M-SHADOW-SNAPSHOT, R-SHADOW-RAM-TRIM
// END_MODULE_CONTRACT
// START_MODULE_MAP
// Snapshot::take: choose legacy or opt-in allocation based on the runtime flag.
// Snapshot::read_ptr and write_ptr: check VM access for the given range.
// Snapshot::pin and exclusive_backing: prevent a raw worker's pages from being discarded.
// END_MODULE_MAP

// START_BLOCK_BACKING
pub(super) enum Snapshot {
    /// An empty Vec with initialized bytes only in captured ranges of its capacity.
    Legacy(Vec<u8>),
    Trimmed {
        backing: Arc<Mutex<SnapshotBuffer>>,
        settled_ms: u64,
    },
}

/// Keeps opt-in backing alive and out of the pool while a worker uses raw bytes.
pub(super) struct SnapshotPin {
    _backing: Arc<Mutex<SnapshotBuffer>>,
}

impl Snapshot {
    pub(super) fn take(len: usize) -> Self {
        if !enabled() {
            return Self::Legacy(pooled(len));
        }
        Self::trimmed(snapshot_pool::take(len).unwrap_or_else(|| SnapshotBuffer::new_zeroed(len)))
    }

    pub(super) fn trimmed(backing: SnapshotBuffer) -> Self {
        Self::Trimmed { backing: Arc::new(Mutex::new(backing)), settled_ms: 0 }
    }

    pub(super) fn recycle(self) {
        match self {
            Self::Legacy(bytes) => give_back(bytes),
            Self::Trimmed { backing, .. } => {
                if let Ok(backing) = Arc::try_unwrap(backing) {
                    snapshot_pool::recycle(backing.into_inner().unwrap_or_else(|e| e.into_inner()));
                }
            }
        }
    }

    pub(super) fn pin(&self) -> Option<SnapshotPin> {
        match self {
            Self::Legacy(_) => None,
            Self::Trimmed { backing, .. } => Some(SnapshotPin { _backing: backing.clone() }),
        }
    }

    pub(super) fn exclusive_backing(&mut self) -> Option<&mut SnapshotBuffer> {
        match self {
            Self::Legacy(_) => None,
            Self::Trimmed { backing, .. } => Arc::get_mut(backing).map(|backing| backing.get_mut().unwrap_or_else(|e| e.into_inner())),
        }
    }

    pub(super) fn mark_settled(&mut self, now: u64) {
        if let Self::Trimmed { settled_ms, .. } = self {
            *settled_ms = now;
        }
    }

    pub(super) fn settled_ms(&self) -> u64 {
        match self {
            Self::Legacy(_) => 0,
            Self::Trimmed { settled_ms, .. } => *settled_ms,
        }
    }
}
// END_BLOCK_BACKING

// START_BLOCK_ACCESS
impl Snapshot {
    // START_CONTRACT: read_ptr/write_ptr
    // PURPOSE: Ensure exactly the required range can be accessed and return the base pointer.
    // INPUTS: A range of shadow offsets; the caller holds the shadow's before-image lock.
    // OUTPUTS: Base pointer, valid to offset and dereference only inside the ensured range.
    // SIDE_EFFECTS: May reclaim idle pool backing and retry commit. Fails loudly on OOM.
    // END_CONTRACT
    /// # Safety
    /// The bytes must have been initialized by a capture or merge. For a pointer used
    /// after dropping the before-image lock, retain the pin and follow the existing
    /// flight ordering and disjoint-access rules.
    pub(super) unsafe fn read_ptr(&mut self, from: usize, to: usize) -> *const u8 {
        match self {
            Self::Legacy(bytes) => bytes.as_ptr(),
            Self::Trimmed { backing, .. } => {
                let mut backing = backing.lock().unwrap_or_else(|e| e.into_inner());
                ensure(&mut backing, from, to);
                // SAFETY: only the range committed above may be accessed; caller supplies the lifetime.
                unsafe { backing.raw_ptr() }
            }
        }
    }

    /// # Safety
    /// The written range must not alias existing accesses. For a pointer used after
    /// dropping the before-image lock, retain the pin until the last access.
    pub(super) unsafe fn write_ptr(&mut self, from: usize, to: usize) -> *mut u8 {
        match self {
            Self::Legacy(bytes) => bytes.as_mut_ptr(),
            Self::Trimmed { backing, .. } => {
                let mut backing = backing.lock().unwrap_or_else(|e| e.into_inner());
                ensure(&mut backing, from, to);
                // SAFETY: only the range committed above may be accessed; caller supplies the lifetime.
                unsafe { backing.raw_mut_ptr() }
            }
        }
    }
}

/// Whether the union of remaining, unsorted declarations still covers a range.
pub(super) fn pending_covers(ranges: &[(usize, usize)], mut from: usize, to: usize) -> bool {
    while from < to {
        let next = ranges.iter().filter(|&&(lo, hi)| lo <= from && from < hi).map(|&(_, hi)| hi).max();
        let Some(next) = next else { return false };
        from = next;
    }
    true
}

fn ensure(backing: &mut SnapshotBuffer, from: usize, to: usize) {
    if backing.ensure_range_committed(from, to) {
        return;
    }
    snapshot_pool::flush();
    if !backing.ensure_range_committed(from, to) {
        crate::host::fail(format_args!(
            "could not back shadow snapshot range {from:#x}..{to:#x} (length {:#x})",
            backing.len(),
        ));
    }
}
// END_BLOCK_ACCESS
