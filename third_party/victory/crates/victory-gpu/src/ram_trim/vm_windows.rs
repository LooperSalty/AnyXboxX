//! Windows backing for exclusively owned, private snapshot reservations.

use std::ffi::c_void;
use std::ops::Range;
use std::ptr::{self, NonNull};
use std::sync::{OnceLock, atomic::Ordering::Relaxed};

use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, GetLastError, SetLastError};
use windows_sys::Win32::System::Memory::{
    GetWriteWatch, MEM_COMMIT, MEM_DECOMMIT, MEM_PRIVATE, MEM_RELEASE, MEM_RESERVE,
    MEMORY_BASIC_INFORMATION, PAGE_NOACCESS, PAGE_READWRITE, VirtualAlloc, VirtualFree,
    VirtualQuery,
};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};

use super::{COUNTERS, VM_PIECE, is_all_zero};

/// Checks that a range is committed private memory in one allocation and was
/// not allocated with write-watch. A successful check does not prove ownership
/// or keep another thread from changing the range. The probe never resets watch bits.
pub fn verify_safe_for_decommit(ptr: *const c_void, len: usize) -> bool {
    if is_private_unwatched(ptr, len) {
        true
    } else {
        COUNTERS.unsafe_decommit_blocked.fetch_add(1, Relaxed);
        false
    }
}

fn is_private_unwatched(ptr: *const c_void, len: usize) -> bool {
    if ptr.is_null() || len == 0 {
        return false;
    }
    let mut cursor = ptr as usize;
    let Some(end) = cursor.checked_add(len) else {
        return false;
    };
    let mut allocation = None;
    while cursor < end {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: VirtualQuery accepts an address to look up and a writable MBI.
        let queried = unsafe { VirtualQuery(cursor as *const _, &mut info, size_of_val(&info)) };
        if queried != size_of_val(&info)
            || info.Type != MEM_PRIVATE
            || info.State != MEM_COMMIT
            || *allocation.get_or_insert(info.AllocationBase) != info.AllocationBase
            || !is_unwatched_page(info.BaseAddress)
        {
            return false;
        }
        let Some(region_end) = (info.BaseAddress as usize).checked_add(info.RegionSize) else {
            return false;
        };
        if region_end <= cursor {
            return false;
        }
        cursor = region_end;
    }
    true
}

fn is_unwatched_page(base: *const c_void) -> bool {
    static PAGE_SIZE: OnceLock<usize> = OnceLock::new();
    let page_size = *PAGE_SIZE.get_or_init(|| {
        // SAFETY: zeroed storage is valid for this Win32 output structure.
        let mut info: SYSTEM_INFO = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is an initialized, writable SYSTEM_INFO.
        unsafe { GetSystemInfo(&mut info) };
        info.dwPageSize as usize
    });
    if page_size == 0 {
        return false;
    }
    let mut dirty = ptr::null_mut();
    let mut count = 1;
    let mut granularity = 0;
    // SAFETY: `base` is a queried committed page boundary; the output has one
    // slot for the one-page range. Flags of zero do not reset watch state.
    let (result, error) = unsafe {
        SetLastError(0);
        let result = GetWriteWatch(0, base, page_size, &mut dirty, &mut count, &mut granularity);
        (result, GetLastError())
    };
    result != 0 && (result == ERROR_INVALID_PARAMETER || error == ERROR_INVALID_PARAMETER)
}

/// The caller must own the whole page-aligned range, with no live references.
unsafe fn safe_decommit(ptr: *mut c_void, bytes: usize) -> bool {
    if !verify_safe_for_decommit(ptr.cast_const(), bytes) {
        return false;
    }
    // SAFETY: the caller owns the pages and the guard validated their VM type.
    unsafe { VirtualFree(ptr, bytes, MEM_DECOMMIT) != 0 }
}

pub(super) enum Release {
    Preserve(usize),
    Zero(usize),
    Discard,
}

// `Reserved(None)` reads as zero after commit; `Reserved(Some(_))` must be
// restored before a slice or checked raw pointer can see the piece.
enum Piece {
    Committed,
    Reserved(Option<Vec<u8>>),
}

pub(super) struct Backing {
    base: NonNull<u8>,
    pieces: Vec<Piece>,
    committed_bytes: usize,
}

// SAFETY: this owns a private reservation and moving it transfers unique access.
unsafe impl Send for Backing {}

impl Backing {
    pub(super) fn new(len: usize) -> Option<Self> {
        let reserve = len.max(1).checked_next_multiple_of(VM_PIECE)?;
        if reserve > isize::MAX as usize {
            return None;
        }
        let count = reserve / VM_PIECE;
        let mut pieces = Vec::new();
        pieces.try_reserve_exact(count).ok()?;
        pieces.resize_with(count, || Piece::Reserved(None));
        // SAFETY: requests a private reservation from the OS, without write-watch.
        let raw = unsafe { VirtualAlloc(ptr::null(), reserve, MEM_RESERVE, PAGE_NOACCESS) };
        let base = NonNull::new(raw.cast::<u8>())?;
        COUNTERS
            .vm_reserved_bytes
            .fetch_add(reserve as u64, Relaxed);
        let mut backing = Self {
            base,
            pieces,
            committed_bytes: 0,
        };
        backing
            .commit_pieces(0..len.div_ceil(VM_PIECE), false)
            .then_some(backing)
    }

    pub(super) fn as_ptr(&self) -> *const u8 {
        self.base.as_ptr().cast_const()
    }

    pub(super) fn as_mut_ptr(&mut self) -> *mut u8 {
        self.base.as_ptr()
    }

    pub(super) fn capacity(&self) -> usize {
        self.pieces.len() * VM_PIECE
    }

    pub(super) fn committed_bytes(&self) -> usize {
        self.committed_bytes
    }

    fn piece_ptr(&self, piece: usize) -> *mut u8 {
        self.base.as_ptr().wrapping_add(piece * VM_PIECE)
    }

    pub(super) fn ensure_pieces(&mut self, pieces: Range<usize>) -> bool {
        self.commit_pieces(pieces, true)
    }

    fn commit_pieces(&mut self, pieces: Range<usize>, recommit: bool) -> bool {
        let mut index = pieces.start;
        while index < pieces.end {
            if matches!(self.pieces[index], Piece::Committed) {
                index += 1;
                continue;
            }
            let start = index;
            index += 1;
            while index < pieces.end && matches!(self.pieces[index], Piece::Reserved(_)) {
                index += 1;
            }
            let bytes = (index - start) * VM_PIECE;
            // SAFETY: the contiguous run is within our reservation and is not write-watched.
            let raw = unsafe {
                VirtualAlloc(
                    self.piece_ptr(start).cast(),
                    bytes,
                    MEM_COMMIT,
                    PAGE_READWRITE,
                )
            };
            if raw.is_null() {
                return false;
            }
            for piece in start..index {
                if let Piece::Reserved(Some(saved)) =
                    std::mem::replace(&mut self.pieces[piece], Piece::Committed)
                {
                    // SAFETY: the destination is newly committed and the saved length
                    // is at most one piece. Heap storage is separate from this reservation.
                    unsafe {
                        saved
                            .as_ptr()
                            .copy_to_nonoverlapping(self.piece_ptr(piece), saved.len())
                    };
                }
            }
            self.committed_bytes += bytes;
            COUNTERS.vm_committed_bytes.fetch_add(bytes as u64, Relaxed);
            if recommit {
                COUNTERS
                    .vm_recommitted_bytes
                    .fetch_add(bytes as u64, Relaxed);
            }
        }
        true
    }

    pub(super) fn release(&mut self, pieces: Range<usize>, mode: Release) -> usize {
        match mode {
            Release::Preserve(len) => pieces.map(|piece| self.preserve_piece(piece, len)).sum(),
            Release::Zero(len) => self.release_runs(pieces, Some(len)),
            Release::Discard => self.release_runs(pieces, None),
        }
    }

    fn preserve_piece(&mut self, piece: usize, len: usize) -> usize {
        if !matches!(self.pieces[piece], Piece::Committed) {
            return 0;
        }
        let data = self.logical_piece(piece, len);
        let saved = if is_all_zero(data) {
            None
        } else {
            let mut saved = Vec::new();
            if saved.try_reserve_exact(data.len()).is_err() {
                return 0;
            }
            saved.extend_from_slice(data);
            Some(saved)
        };
        if self.decommit_run(piece..piece + 1) == 0 {
            return 0;
        }
        self.pieces[piece] = Piece::Reserved(saved);
        VM_PIECE
    }

    // With `zero_len`, release only zero pieces. Otherwise discard even saved pieces.
    fn release_runs(&mut self, pieces: Range<usize>, zero_len: Option<usize>) -> usize {
        let eligible = |backing: &Self, piece| {
            matches!(backing.pieces[piece], Piece::Committed)
                && zero_len.is_none_or(|len| is_all_zero(backing.logical_piece(piece, len)))
        };
        let mut index = pieces.start;
        let mut released = 0;
        while index < pieces.end {
            if !eligible(self, index) {
                if zero_len.is_none() {
                    self.pieces[index] = Piece::Reserved(None);
                }
                index += 1;
                continue;
            }
            let start = index;
            index += 1;
            while index < pieces.end && eligible(self, index) {
                index += 1;
            }
            let bytes = self.decommit_run(start..index);
            if zero_len.is_some() {
                COUNTERS
                    .vm_zero_decommitted_bytes
                    .fetch_add(bytes as u64, Relaxed);
            } else if bytes == 0 {
                // SAFETY: the run is still committed and exclusively owned.
                unsafe {
                    self.piece_ptr(start)
                        .write_bytes(0, (index - start) * VM_PIECE)
                };
            }
            released += bytes;
        }
        released
    }

    fn logical_piece(&self, piece: usize, len: usize) -> &[u8] {
        debug_assert!(matches!(self.pieces[piece], Piece::Committed));
        let bytes = (len - piece * VM_PIECE).min(VM_PIECE);
        // SAFETY: both callers check that the piece is committed and inside `len`.
        unsafe { std::slice::from_raw_parts(self.piece_ptr(piece), bytes) }
    }

    fn decommit_run(&mut self, pieces: Range<usize>) -> usize {
        let bytes = pieces.len() * VM_PIECE;
        // SAFETY: piece boundaries are page-aligned inside the exclusive reservation.
        // Only committed pieces without outstanding references are passed here.
        if !unsafe { safe_decommit(self.piece_ptr(pieces.start).cast(), bytes) } {
            return 0;
        }
        self.pieces[pieces].fill_with(|| Piece::Reserved(None));
        self.committed_bytes -= bytes;
        COUNTERS.vm_committed_bytes.fetch_sub(bytes as u64, Relaxed);
        COUNTERS
            .vm_decommitted_bytes
            .fetch_add(bytes as u64, Relaxed);
        bytes
    }
}

impl Drop for Backing {
    fn drop(&mut self) {
        // SAFETY: releasing the original base returned for this private reservation.
        unsafe { VirtualFree(self.base.as_ptr().cast(), 0, MEM_RELEASE) };
        COUNTERS
            .vm_reserved_bytes
            .fetch_sub(self.capacity() as u64, Relaxed);
        COUNTERS
            .vm_committed_bytes
            .fetch_sub(self.committed_bytes as u64, Relaxed);
    }
}
