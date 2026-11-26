//! Graphics regions and `XMemVirtualQuery` (port of `runtime/xmem/era_query.cpp`).
//!
//! Adapted from Xbox2Windows, `src/era_query.h` and `src/era_query.cpp`
//! (https://github.com/R0Y00/Xbox2Windows), under the MIT License, Copyright (c) 2026
//! Xbox2Windows contributors; full notice in `lib.rs` and THIRD_PARTY.md.
//!
//! Shape seen at the four BVH calls Xbox2Windows studied: process, address, a zeroed 64-byte
//! buffer, no length; the 32-bit result is tested. Bytes 0x00-0x2F follow
//! MEMORY_BASIC_INFORMATION (State read at +0x20, Protect at +0x24). Byte +0x30 has no public
//! definition: here bit 0 = the address lies in a registered graphics region. That is the PC's
//! policy, not a confirmed Series X field.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::{Mutex, MutexGuard};

use windows_sys::Win32::Foundation::{ERROR_BAD_LENGTH, ERROR_NOACCESS, HANDLE, SetLastError};
use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQueryEx};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetProcessId};

pub(crate) const TAIL_OFFSET: usize = 0x30;
pub(crate) const TAIL_GRAPHICS: u8 = 0x01;
const _: () = assert!(size_of::<MEMORY_BASIC_INFORMATION>() == TAIL_OFFSET);

/// Start -> end (excluded) of each registered region; they never overlap.
static REGIONS: Mutex<BTreeMap<usize, usize>> = Mutex::new(BTreeMap::new());

fn regions() -> MutexGuard<'static, BTreeMap<usize, usize>> {
    // A panic while the map was held leaves it consistent (each change is one call).
    REGIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn is_current_process(process: HANDLE) -> bool {
    // SAFETY: plain queries; GetProcessId fails (0) on a handle it cannot read.
    unsafe { process == GetCurrentProcess() || GetProcessId(process) == GetCurrentProcessId() }
}

fn is_registered(address: usize) -> bool {
    regions()
        .range(..=address)
        .next_back()
        .is_some_and(|(_, &end)| address < end)
}

/// False for a null, empty or wrapping region, or one that overlaps a registered region.
pub(crate) fn register_graphics_region(base: *const c_void, size: usize) -> bool {
    let begin = base as usize;
    let Some(end) = begin.checked_add(size) else {
        return false;
    };
    if base.is_null() || size == 0 {
        return false;
    }
    let mut regions = regions();
    let next_overlaps = regions
        .range(begin..)
        .next()
        .is_some_and(|(&start, _)| start < end);
    let previous_overlaps = regions
        .range(..begin)
        .next_back()
        .is_some_and(|(_, &previous_end)| previous_end > begin);
    if next_overlaps || previous_overlaps {
        return false;
    }
    regions.insert(begin, end);
    true
}

pub(crate) fn unregister_graphics_region(base: *const c_void) -> bool {
    regions().remove(&(base as usize)).is_some()
}

/// Writes MEMORY_BASIC_INFORMATION into `buffer[0, 0x30)` and, if `length > 0x30`, the byte at
/// +0x30; later bytes are left untouched. Returns the MEMORY_BASIC_INFORMATION bytes written,
/// or 0 on failure with the last error set (as VirtualQueryEx).
///
/// # Safety
/// `buffer` is null or points at `length` writable bytes.
pub(crate) unsafe fn query(
    process: HANDLE,
    address: *const c_void,
    buffer: *mut c_void,
    length: usize,
) -> usize {
    let size = size_of::<MEMORY_BASIC_INFORMATION>();
    if length < size {
        // SAFETY: plain thread-local state.
        unsafe { SetLastError(ERROR_BAD_LENGTH) };
        return 0;
    }
    if buffer.is_null() {
        // SAFETY: plain thread-local state.
        unsafe { SetLastError(ERROR_NOACCESS) };
        return 0;
    }
    // SAFETY: plain data; zero is a valid value, and VirtualQueryEx fills it.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: `info` is a sized local; the process handle is the caller's.
    if unsafe { VirtualQueryEx(process, address, &mut info, size) } != size {
        return 0;
    }
    let bytes = buffer.cast::<u8>();
    // SAFETY: the buffer holds `length` >= `size` bytes per this function's contract.
    unsafe { std::ptr::copy_nonoverlapping((&raw const info).cast::<u8>(), bytes, size) };
    if length > TAIL_OFFSET {
        let graphics = is_current_process(process) && is_registered(address as usize);
        let tail = if graphics { TAIL_GRAPHICS } else { 0 };
        // SAFETY: as above, byte +0x30 is inside the buffer.
        unsafe { bytes.add(TAIL_OFFSET).write(tail) };
    }
    size
}
