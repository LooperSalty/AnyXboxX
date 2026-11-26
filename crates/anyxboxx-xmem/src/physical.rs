//! Physical pages of xmem.dll (port of `runtime/xmem/physical_pages.cpp`):
//! XMemAllocatePhysicalPages, XMemFreePhysicalPages, XMemMapPhysicalPages and
//! XMemMapPhysicalPagesScatter, plus the two tools-memory calls.
//!
//! The PC has no physical pages to share with the title. The page numbers issued here are
//! identifiers of our own table, checked at every call; a mapping commits its own 64 KiB of
//! memory per page. Two mappings of one page are therefore NOT aliases: a write through one does
//! not show in the other. That is the limit of this version (VIctory's own physical pages alias
//! through file sections, but need placeholder reservations; see `lib.rs`).
//!
//! Shapes: XMemAllocatePhysicalPages(attributes, *count, *array) with the count in 64 KiB pages
//! (in and out); XMemFreePhysicalPages(count, *array); XMemMapPhysicalPages(address, count,
//! *array) returns the mapped address. XMemMapPhysicalPagesScatter: shape assumed identical (to
//! check on an executable that calls it).

use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::{Mutex, MutexGuard};

use victory_hle::modules::xmem::EraFields;
use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED, SetLastError};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc};
use windows_sys::core::BOOL;

use crate::log::{log, trace};

const PAGE_BYTES: usize = 64 << 10;
/// 4 GiB of pages at most per call.
const MAX_PAGES: usize = 0x1_0000;
/// Small numbers are never issued.
const FIRST_FRAME: usize = 0x100;

struct Frames {
    next: usize,
    live: HashSet<usize>,
}

static FRAMES: Mutex<Option<Frames>> = Mutex::new(None);

fn with_frames<T>(work: impl FnOnce(&mut Frames) -> T) -> T {
    let mut guard: MutexGuard<'_, Option<Frames>> = FRAMES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let frames = guard.get_or_insert_with(|| Frames {
        next: FIRST_FRAME,
        live: HashSet::new(),
    });
    work(frames)
}

fn set_error(error: u32) {
    // SAFETY: plain thread-local state.
    unsafe { SetLastError(error) };
}

/// The caller's page-number array, if the request is well formed.
///
/// # Safety
/// A non-null `frames` points at `count` page numbers.
unsafe fn request<'a>(count: usize, frames: *const usize) -> Option<&'a [usize]> {
    if frames.is_null() || count == 0 || count > MAX_PAGES {
        return None;
    }
    // SAFETY: per this function's contract.
    Some(unsafe { std::slice::from_raw_parts(frames, count) })
}

fn all_live(frames: &Frames, numbers: &[usize]) -> bool {
    numbers.iter().all(|number| frames.live.contains(number))
}

fn all_live_and_unique(frames: &Frames, numbers: &[usize]) -> bool {
    let mut seen = HashSet::with_capacity(numbers.len());
    numbers
        .iter()
        .all(|number| frames.live.contains(number) && seen.insert(*number))
}

/// # Safety
/// `count` and `frames` as in [`request`].
unsafe fn map_frames(address: *mut c_void, count: usize, frames: *const usize) -> *mut c_void {
    // SAFETY: forwarded contract.
    let Some(numbers) = (unsafe { request(count, frames) }) else {
        set_error(ERROR_INVALID_PARAMETER);
        return std::ptr::null_mut();
    };
    if !with_frames(|table| all_live(table, numbers)) {
        log!("[xmem] XMemMapPhysicalPages: unknown page number, refused");
        set_error(ERROR_INVALID_PARAMETER);
        return std::ptr::null_mut();
    }
    let bytes = count * PAGE_BYTES;
    let kind = if address.is_null() {
        MEM_RESERVE | MEM_COMMIT
    } else {
        MEM_COMMIT
    };
    // SAFETY: the title's own address (or none), committed read-write.
    let mapped = unsafe { VirtualAlloc(address, bytes, kind, PAGE_READWRITE) };
    trace!("[xmem] XMemMapPhysicalPages {count} pages -> {mapped:p} (no alias between mappings)");
    mapped
}

/// # Safety
/// `count` points at a page count and `frames` at room for that many page numbers.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn XMemAllocatePhysicalPages(
    attributes: u64,
    count: *mut usize,
    frames: *mut usize,
) -> BOOL {
    // SAFETY: a non-null count is readable per this function's contract.
    let wanted = unsafe { count.as_ref() }.copied();
    let valid = wanted.filter(|&n| {
        EraFields::decode(attributes).reserved == 0 && !frames.is_null() && n > 0 && n <= MAX_PAGES
    });
    let Some(wanted) = valid else {
        set_error(ERROR_INVALID_PARAMETER);
        return 0;
    };
    // SAFETY: room for `wanted` page numbers per this function's contract.
    let out = unsafe { std::slice::from_raw_parts_mut(frames, wanted) };
    with_frames(|table| {
        for slot in out.iter_mut() {
            *slot = table.next;
            table.live.insert(table.next);
            table.next += 1;
        }
    });
    trace!("[xmem] XMemAllocatePhysicalPages {wanted} pages");
    1
}

/// # Safety
/// `frames` points at `count` page numbers.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn XMemFreePhysicalPages(count: usize, frames: *const usize) -> BOOL {
    // SAFETY: forwarded contract.
    let Some(numbers) = (unsafe { request(count, frames) }) else {
        set_error(ERROR_INVALID_PARAMETER);
        return 0;
    };
    let freed = with_frames(|table| {
        let live = all_live_and_unique(table, numbers);
        if live {
            numbers.iter().for_each(|number| {
                table.live.remove(number);
            });
        }
        live
    });
    if !freed {
        set_error(ERROR_INVALID_PARAMETER);
    }
    freed.into()
}

/// # Safety
/// `frames` points at `count` page numbers; `address` is null or the title's reservation.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn XMemMapPhysicalPages(
    address: *mut c_void,
    count: usize,
    frames: *const usize,
) -> *mut c_void {
    // SAFETY: forwarded contract.
    unsafe { map_frames(address, count, frames) }
}

/// # Safety
/// As [`XMemMapPhysicalPages`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn XMemMapPhysicalPagesScatter(
    address: *mut c_void,
    count: usize,
    frames: *const usize,
) -> *mut c_void {
    // SAFETY: forwarded contract.
    unsafe { map_frames(address, count, frames) }
}

/// Tools memory: absent from a retail console (VIctory's answer, reused).
#[unsafe(no_mangle)]
pub extern "system" fn XMemIsToolsMemAvailable() -> BOOL {
    victory_hle::modules::xmem::XMemIsToolsMemAvailable()
}

/// Not served: the shape of XMemTransferMemory is not established here. A logged error, never a
/// success (VIctory's version takes a kind and a size and moves title budget; see `lib.rs`).
#[unsafe(no_mangle)]
pub extern "system" fn XMemTransferMemory() -> BOOL {
    log!("[xmem] XMemTransferMemory not implemented");
    set_error(ERROR_NOT_SUPPORTED);
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::GetLastError;

    #[test]
    fn free_physical_pages_refuses_duplicate_frame_numbers_atomically() {
        let mut count = 1usize;
        let mut frame = [0usize; 1];
        // SAFETY: valid single-element output buffer.
        assert_eq!(
            unsafe { XMemAllocatePhysicalPages(0x280, &mut count, frame.as_mut_ptr()) },
            1
        );
        let duplicate = [frame[0], frame[0]];
        set_error(0);
        // SAFETY: valid 2-element slice with duplicate frame number.
        assert_eq!(
            unsafe { XMemFreePhysicalPages(duplicate.len(), duplicate.as_ptr()) },
            0
        );
        // SAFETY: plain thread-local state.
        assert_eq!(unsafe { GetLastError() }, ERROR_INVALID_PARAMETER);
        // The frame must still be live after the refused duplicate free.
        // SAFETY: valid single-element slice.
        assert_eq!(unsafe { XMemFreePhysicalPages(1, frame.as_ptr()) }, 1);
    }
}
