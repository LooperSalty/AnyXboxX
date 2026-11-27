//! XMemVirtualAlloc: the console's virtual allocation, translated to VirtualAlloc (port of
//! `runtime/xmem/virtual_alloc.cpp`).
//!
//! Shape measured at first boot (docs/research/first-boot-1021.md, section 4):
//! XMemVirtualAlloc(address, size, allocation type, attributes, protection). The type carries
//! the MEM_* flags and XMEM_LARGE_PAGES (0x20000000); the attributes are ERA labels (object type,
//! allocator id) without effect here, except the reserved bits, which are refused; the
//! protection carries a PAGE_* in its low byte, the other bits (cache, placement) are not held
//! by the PC. 2 MiB pages are not held: the region is aligned on 2 MiB and committed in ordinary
//! memory. Freeing: VirtualFree, as for any host reservation.

use std::ffi::c_void;

use victory_hle::modules::xmem::EraFields;
use windows_sys::Win32::Foundation::{
    ERROR_INVALID_PARAMETER, ERROR_NOT_ENOUGH_MEMORY, GetLastError, SetLastError,
};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
    PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE,
    PAGE_WRITECOPY, VirtualAlloc,
};

use crate::log::log;
use crate::pages::allocate_aligned;

/// The host's MEM_TOP_DOWN (not in windows-sys' memory module).
const MEM_TOP_DOWN: u32 = 0x10_0000;
/// XMEM_LARGE_PAGES, also the host's MEM_LARGE_PAGES.
const XMEM_LARGE_PAGES: u32 = 0x2000_0000;
/// The other allocation flags are not held.
const ALLOCATION_TYPE_MASK: u32 = MEM_COMMIT | MEM_RESERVE | MEM_TOP_DOWN;
const PROTECTION_BASE_MASK: u32 = 0xff;
const PAGE_PROTECTIONS: [u32; 8] = [
    PAGE_NOACCESS,
    PAGE_READONLY,
    PAGE_READWRITE,
    PAGE_WRITECOPY,
    PAGE_EXECUTE,
    PAGE_EXECUTE_READ,
    PAGE_EXECUTE_READWRITE,
    PAGE_EXECUTE_WRITECOPY,
];
/// The console's granularity for 64 KiB pages.
const PAGE_GRANULARITY: usize = 64 << 10;
/// XMEM_LARGE_PAGES: 2 MiB.
const LARGE_PAGE_BYTES: usize = 2 << 20;

/// The host's allocation type: at least one of MEM_COMMIT and MEM_RESERVE.
fn win32_type(kind: u32) -> Option<u32> {
    let mapped = kind & ALLOCATION_TYPE_MASK;
    (mapped & (MEM_COMMIT | MEM_RESERVE) != 0).then_some(mapped)
}

/// The host's protection: a known PAGE_* in the low byte, plus PAGE_GUARD if asked.
fn win32_protect(protect: u32) -> Option<u32> {
    let base = protect & PROTECTION_BASE_MASK;
    PAGE_PROTECTIONS
        .contains(&base)
        .then_some(base | (protect & PAGE_GUARD))
}

fn alignment(kind: u32) -> usize {
    if kind & XMEM_LARGE_PAGES != 0 {
        LARGE_PAGE_BYTES
    } else {
        PAGE_GRANULARITY
    }
}

fn round_up(value: usize, unit: usize) -> Option<usize> {
    value.checked_add(unit - 1).map(|v| v & !(unit - 1))
}

fn no_memory() -> *mut c_void {
    // SAFETY: plain thread-local state.
    unsafe { SetLastError(ERROR_NOT_ENOUGH_MEMORY) };
    std::ptr::null_mut()
}

/// At a requested address: the range is widened to the alignment, as VirtualAlloc does.
fn at_address(
    address: *mut c_void,
    size: usize,
    alignment: usize,
    kind: u32,
    protect: u32,
) -> *mut c_void {
    let start = address as usize & !(alignment - 1);
    let Some(end) = (address as usize)
        .checked_add(size)
        .and_then(|end| round_up(end, alignment))
    else {
        return no_memory();
    };
    // SAFETY: the title's own range, with host flags.
    unsafe { VirtualAlloc(start as *const c_void, end - start, kind, protect) }
}

fn refuse(kind: u32, protect: u32) -> *mut c_void {
    log!("[xmem] XMemVirtualAlloc: type {kind:#x}, protection {protect:#x} undefined");
    // SAFETY: plain thread-local state.
    unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
    std::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "system" fn XMemVirtualAlloc(
    address: *mut c_void,
    size: usize,
    kind: u32,
    attributes: u64,
    protect: u32,
) -> *mut c_void {
    let (Some(host_kind), Some(host_protect)) = (win32_type(kind), win32_protect(protect)) else {
        return refuse(kind, protect);
    };
    if EraFields::decode(attributes).reserved != 0 {
        return refuse(kind, protect);
    }
    let alignment = alignment(kind);
    let block = if address.is_null() {
        let Some(bytes) = round_up(size.max(1), alignment) else {
            return no_memory();
        };
        allocate_aligned(bytes, alignment, host_kind, host_protect)
    } else {
        at_address(address, size, alignment, host_kind, host_protect)
    };
    if block.is_null() {
        // SAFETY: plain thread-local state, kept across the log line.
        let error = unsafe { GetLastError() };
        log!("[xmem] XMemVirtualAlloc {size} bytes, type {kind:#x} -> NULL (error {error})");
        // SAFETY: as above.
        unsafe { SetLastError(error) };
    }
    block
}
