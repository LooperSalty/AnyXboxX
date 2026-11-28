//! Small heap and large VM snapshots under the same checked access API.

use std::ops::Range;

use super::vm::{in_bounds, whole_pieces, zero_pieces};
use super::{VM_THRESHOLD, VmBlock, enabled, note_pool_trimmed};

/// Snapshot storage. A small heap buffer does not release VM commit independently.
pub enum SnapshotBuffer {
    Small(Vec<u8>),
    Large(VmBlock),
}

impl SnapshotBuffer {
    /// Allocates a zeroed snapshot, preferring VM backing at and above [`VM_THRESHOLD`].
    pub fn new_zeroed(len: usize) -> Self {
        if len >= VM_THRESHOLD
            && let Some(block) = VmBlock::new_zeroed(len)
        {
            return Self::Large(block);
        }
        Self::Small(vec![0; len])
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Small(bytes) => bytes.len(),
            Self::Large(block) => block.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn capacity(&self) -> usize {
        match self {
            Self::Small(bytes) => bytes.capacity(),
            Self::Large(block) => block.capacity(),
        }
    }

    pub fn committed_bytes(&self) -> usize {
        match self {
            Self::Small(bytes) => bytes.capacity(),
            Self::Large(block) => block.committed_bytes(),
        }
    }

    pub const fn is_large(&self) -> bool {
        matches!(self, Self::Large(_))
    }

    /// Base address of a large block for identity or diagnostics.
    pub fn base_addr(&self) -> Option<usize> {
        match self {
            Self::Small(_) => None,
            Self::Large(block) => Some(block.base_addr()),
        }
    }

    /// Validates the bounds and restores backing before an access.
    pub fn ensure_commit(&mut self, range: Range<usize>) -> bool {
        match self {
            Self::Small(bytes) => in_bounds(bytes.len(), &range),
            Self::Large(block) => block.ensure_commit(range),
        }
    }

    /// Two-offset form for callers that track `[start, end)` separately.
    pub fn ensure_range_committed(&mut self, start: usize, end: usize) -> bool {
        self.ensure_commit(start..end)
    }

    pub fn as_slice_committed(&mut self, range: Range<usize>) -> Option<&[u8]> {
        match self {
            Self::Small(bytes) => bytes.get(range),
            Self::Large(block) => block.as_slice_committed(range),
        }
    }

    pub fn as_mut_slice_committed(&mut self, range: Range<usize>) -> Option<&mut [u8]> {
        match self {
            Self::Small(bytes) => bytes.get_mut(range),
            Self::Large(block) => block.as_mut_slice_committed(range),
        }
    }

    /// Reads a committed range into a destination of exactly the same length.
    /// On failure, `dst` stays unchanged.
    pub fn read_committed_range(&mut self, range: Range<usize>, dst: &mut [u8]) -> bool {
        match self {
            Self::Large(block) => block.read_committed_range(range, dst),
            Self::Small(bytes) => {
                let Some(src) = bytes.get(range).filter(|src| src.len() == dst.len()) else {
                    return false;
                };
                dst.copy_from_slice(src);
                true
            }
        }
    }

    /// Writes `src` starting at `at`, preserving the surrounding logical bytes.
    pub fn write_committed_range(&mut self, at: usize, src: &[u8]) -> bool {
        match self {
            Self::Large(block) => block.write_committed_range(at, src),
            Self::Small(bytes) => {
                let Some(dst) = at
                    .checked_add(src.len())
                    .and_then(|end| bytes.get_mut(at..end))
                else {
                    return false;
                };
                dst.copy_from_slice(src);
                true
            }
        }
    }

    /// Returns a raw base pointer for an already checked and committed range.
    ///
    /// # Safety
    /// The caller must follow the range, lifetime, and aliasing rules in
    /// [`VmBlock::raw_ptr`], including on small buffers.
    pub unsafe fn raw_ptr(&self) -> *const u8 {
        match self {
            Self::Small(bytes) => bytes.as_ptr(),
            // SAFETY: the caller provides the required guarantees for this block.
            Self::Large(block) => unsafe { block.raw_ptr() },
        }
    }

    /// Returns a writable raw base pointer for an already checked and committed range.
    ///
    /// # Safety
    /// The caller must follow the range, lifetime, and aliasing rules in
    /// [`VmBlock::raw_mut_ptr`], including on small buffers.
    pub unsafe fn raw_mut_ptr(&mut self) -> *mut u8 {
        match self {
            Self::Small(bytes) => bytes.as_mut_ptr(),
            // SAFETY: the caller provides the required guarantees for this block.
            Self::Large(block) => unsafe { block.raw_mut_ptr() },
        }
    }

    /// Resizes within capacity, keeping the retained prefix and zeroing newly exposed bytes.
    pub fn prepare_for_overwrite(&mut self, new_len: usize) -> bool {
        match self {
            Self::Small(bytes) if new_len <= bytes.capacity() => {
                bytes.resize(new_len, 0);
                true
            }
            Self::Small(_) => false,
            Self::Large(block) => block.prepare_for_overwrite(new_len),
        }
    }

    /// Releases VM commit without losing logical bytes; no-op for a heap buffer.
    pub fn decommit_all(&mut self) -> usize {
        match self {
            Self::Small(_) => 0,
            Self::Large(block) => block.decommit_all(),
        }
    }

    /// Releases VM commit for wholly contained pieces without losing logical bytes.
    pub fn decommit_byte_range(&mut self, start: usize, end: usize) -> usize {
        match self {
            Self::Small(_) => 0,
            Self::Large(block) => block.decommit_byte_range(start, end),
        }
    }

    /// Releases VM commit for zero pieces; no-op for a heap buffer.
    pub fn decommit_zero_pieces(&mut self) -> usize {
        match self {
            Self::Small(_) => 0,
            Self::Large(block) => block.decommit_zero_pieces(),
        }
    }

    /// Explicitly drops the previous contents, returning newly decommitted VM bytes.
    pub fn discard_all(&mut self) -> usize {
        match self {
            Self::Small(bytes) => {
                bytes.fill(0);
                0
            }
            Self::Large(block) => block.discard_all(),
        }
    }

    /// Explicitly drops the contents of wholly contained pieces.
    pub fn discard_byte_range(&mut self, start: usize, end: usize) -> usize {
        match self {
            Self::Small(bytes) => {
                let pieces = whole_pieces(bytes.len(), start..end);
                zero_pieces(bytes, pieces);
                0
            }
            Self::Large(block) => block.discard_byte_range(start, end),
        }
    }
}

/// Checks for zero bytes using aligned words for the bulk of the slice.
pub fn is_all_zero(bytes: &[u8]) -> bool {
    // SAFETY: every bit pattern of the aligned part is valid as `u64`.
    let (head, words, tail) = unsafe { bytes.align_to::<u64>() };
    head.iter().all(|&b| b == 0) && words.iter().all(|&w| w == 0) && tail.iter().all(|&b| b == 0)
}

/// Clears scratch storage for reuse. Capacity trimming requires the runtime opt-in.
pub fn maybe_shrink_scratch_vec(buf: &mut Vec<u8>, needed: usize, cap_ceiling: usize) {
    buf.clear();
    let before = buf.capacity();
    if enabled() && before > cap_ceiling && before > needed.saturating_mul(4) {
        buf.shrink_to(needed.max(cap_ceiling / 2));
        note_pool_trimmed((before - buf.capacity()) as u64);
    }
}
