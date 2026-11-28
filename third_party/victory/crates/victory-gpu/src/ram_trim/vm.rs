//! Owned snapshot storage with checked read-through access.

use std::ops::Range;

use super::VM_PIECE;
#[cfg(windows)]
pub use super::vm_windows::verify_safe_for_decommit;
#[cfg(windows)]
use super::vm_windows::{Backing, Release};

/// Non-Windows hosts do not support this decommit implementation.
#[cfg(not(windows))]
pub fn verify_safe_for_decommit(_ptr: *const std::ffi::c_void, _len: usize) -> bool {
    false
}

/// Private Windows reservation, or ordinary heap storage on other hosts.
/// Releasing VM commit does not necessarily reduce process memory: live bytes
/// in nonzero pieces need a heap copy until the pieces are restored.
pub struct VmBlock {
    len: usize,
    #[cfg(windows)]
    backing: Backing,
    #[cfg(not(windows))]
    backing: Vec<u8>,
}

impl VmBlock {
    /// Allocates `len` zeroed logical bytes.
    pub fn new_zeroed(len: usize) -> Option<Self> {
        #[cfg(windows)]
        let backing = Backing::new(len)?;
        #[cfg(not(windows))]
        let backing = {
            let mut bytes = Vec::new();
            bytes.try_reserve_exact(len).ok()?;
            bytes.resize(len, 0);
            bytes
        };
        Some(Self { len, backing })
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bytes reserved by the backing, including final-piece padding on Windows.
    pub fn capacity(&self) -> usize {
        self.backing.capacity()
    }

    /// Bytes committed in the reservation (heap capacity on non-Windows hosts).
    pub fn committed_bytes(&self) -> usize {
        #[cfg(windows)]
        let bytes = self.backing.committed_bytes();
        #[cfg(not(windows))]
        let bytes = self.backing.capacity();
        bytes
    }

    /// Number of pieces covering the logical length.
    pub fn piece_count(&self) -> usize {
        self.len.div_ceil(VM_PIECE)
    }

    /// Base address for identity or diagnostics, not a dereferenceable pointer.
    pub fn base_addr(&self) -> usize {
        self.backing.as_ptr() as usize
    }

    /// Recommits and restores all pieces touched by a logical byte range.
    /// Returns false on invalid bounds or commit failure. On commit failure,
    /// earlier pieces may already have been restored; it is safe to retry.
    pub fn ensure_commit(&mut self, range: Range<usize>) -> bool {
        if !in_bounds(self.len, &range) {
            return false;
        }
        #[cfg(windows)]
        if !range.is_empty() {
            return self
                .backing
                .ensure_pieces(range.start / VM_PIECE..range.end.div_ceil(VM_PIECE));
        }
        true
    }

    /// Two-offset form for callers that track `[start, end)` separately.
    pub fn ensure_range_committed(&mut self, start: usize, end: usize) -> bool {
        self.ensure_commit(start..end)
    }

    /// Recommits and restores a range before borrowing it.
    pub fn as_slice_committed(&mut self, range: Range<usize>) -> Option<&[u8]> {
        self.as_mut_slice_committed(range).map(|slice| &*slice)
    }

    /// Recommits and restores a range before borrowing it mutably.
    pub fn as_mut_slice_committed(&mut self, range: Range<usize>) -> Option<&mut [u8]> {
        if !self.ensure_commit(range.clone()) {
            return None;
        }
        // SAFETY: the validated range lies in the logical, initialized backing
        // and is committed. The exclusive borrow prevents concurrent access or release.
        Some(unsafe {
            std::slice::from_raw_parts_mut(
                self.backing.as_mut_ptr().add(range.start),
                range.end - range.start,
            )
        })
    }

    /// Reads a committed range into a destination of exactly the same length.
    /// On failure, `dst` stays unchanged.
    pub fn read_committed_range(&mut self, range: Range<usize>, dst: &mut [u8]) -> bool {
        if range.end.checked_sub(range.start) != Some(dst.len()) {
            return false;
        }
        let Some(src) = self.as_slice_committed(range) else {
            return false;
        };
        dst.copy_from_slice(src);
        true
    }

    /// Writes `src` starting at `at`, restoring any untouched bytes in the same pieces.
    pub fn write_committed_range(&mut self, at: usize, src: &[u8]) -> bool {
        let Some(end) = at.checked_add(src.len()) else {
            return false;
        };
        let Some(dst) = self.as_mut_slice_committed(at..end) else {
            return false;
        };
        dst.copy_from_slice(src);
        true
    }

    /// Returns the base pointer for callers that need raw access.
    ///
    /// # Safety
    /// Access only logical bytes in a range successfully ensured on this block.
    /// Do not use the pointer after release, resize, or drop, or while access
    /// would conflict with Rust borrows.
    pub unsafe fn raw_ptr(&self) -> *const u8 {
        self.backing.as_ptr()
    }

    /// Returns the writable base pointer for callers that need raw access.
    ///
    /// # Safety
    /// The same range, lifetime, and aliasing rules as [`Self::raw_ptr`] apply.
    pub unsafe fn raw_mut_ptr(&mut self) -> *mut u8 {
        self.backing.as_mut_ptr()
    }

    /// Resizes within capacity, restoring the retained prefix and zeroing newly
    /// exposed bytes. Whole pieces past the new length are discarded on Windows.
    pub fn prepare_for_overwrite(&mut self, new_len: usize) -> bool {
        if new_len > self.capacity() {
            return false;
        }
        #[cfg(windows)]
        {
            let needed = new_len.div_ceil(VM_PIECE);
            if !self.backing.ensure_pieces(0..needed) {
                return false;
            }
            if new_len > self.len {
                // SAFETY: the grown prefix was just committed within the reservation.
                unsafe {
                    self.backing
                        .as_mut_ptr()
                        .add(self.len)
                        .write_bytes(0, new_len - self.len);
                }
            }
            let pieces = self.capacity() / VM_PIECE;
            self.backing.release(needed..pieces, Release::Discard);
        }
        #[cfg(not(windows))]
        self.backing.resize(new_len, 0);
        self.len = new_len;
        true
    }

    /// Releases committed pieces while preserving logical bytes for future reads.
    /// If a nonzero piece cannot be saved, it stays committed. Returns VM bytes
    /// decommitted, not net RAM saved. Use [`Self::discard_all`] for dead data.
    pub fn decommit_all(&mut self) -> usize {
        #[cfg(windows)]
        {
            self.backing
                .release(0..self.piece_count(), Release::Preserve(self.len))
        }
        #[cfg(not(windows))]
        {
            0
        }
    }

    /// Preserves and decommits pieces wholly covered by `[start, end)`.
    /// Final-piece padding is ignored. Returns VM bytes decommitted.
    pub fn decommit_byte_range(&mut self, start: usize, end: usize) -> usize {
        #[cfg(windows)]
        {
            self.backing.release(
                whole_pieces(self.len, start..end),
                Release::Preserve(self.len),
            )
        }
        #[cfg(not(windows))]
        {
            let _ = (start, end);
            0
        }
    }

    /// Decommits committed pieces containing only logical zeros, without a heap copy.
    pub fn decommit_zero_pieces(&mut self) -> usize {
        #[cfg(windows)]
        {
            self.backing
                .release(0..self.piece_count(), Release::Zero(self.len))
        }
        #[cfg(not(windows))]
        {
            0
        }
    }

    /// Explicitly drops contents, including any saved copies. Later reads return
    /// zeros for discarded pieces. Returns newly decommitted VM bytes.
    pub fn discard_all(&mut self) -> usize {
        #[cfg(windows)]
        {
            self.backing
                .release(0..self.piece_count(), Release::Discard)
        }
        #[cfg(not(windows))]
        {
            self.backing.fill(0);
            0
        }
    }

    /// Discards whole pieces covered by `[start, end)`. Final-piece padding is ignored.
    pub fn discard_byte_range(&mut self, start: usize, end: usize) -> usize {
        let pieces = whole_pieces(self.len, start..end);
        #[cfg(windows)]
        {
            self.backing.release(pieces, Release::Discard)
        }
        #[cfg(not(windows))]
        {
            zero_pieces(&mut self.backing, pieces);
            0
        }
    }
}

pub(super) fn in_bounds(len: usize, range: &Range<usize>) -> bool {
    range.start <= range.end && range.end <= len
}

pub(super) fn whole_pieces(len: usize, range: Range<usize>) -> Range<usize> {
    let end = range.end.min(len);
    if range.start >= end {
        return 0..0;
    }
    let first = range.start.div_ceil(VM_PIECE);
    let last = if end == len {
        end.div_ceil(VM_PIECE)
    } else {
        end / VM_PIECE
    };
    first..last.max(first)
}

pub(super) fn zero_pieces(bytes: &mut [u8], pieces: Range<usize>) {
    if !pieces.is_empty() {
        let end = bytes.len().min(pieces.end.saturating_mul(VM_PIECE));
        bytes[pieces.start * VM_PIECE..end].fill(0);
    }
}
