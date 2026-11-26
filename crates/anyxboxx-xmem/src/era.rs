//! Placement of `XMemAlloc` blocks from their `XALLOC_ATTRIBUTES` (port of
//! `runtime/xmem/era_heap.cpp`).
//!
//! Adapted from Xbox2Windows, `src/era_heap.h` and `src/era_heap.cpp`
//! (https://github.com/R0Y00/Xbox2Windows), under the MIT License, Copyright (c) 2026
//! Xbox2Windows contributors; full notice in `lib.rs` and THIRD_PARTY.md.
//!
//! The field split itself is VIctory's [`EraFields`] (decision D3). Placement:
//! - heap (type 0): Rust's allocator at the requested alignment, or an own reservation beyond a
//!   page;
//! - graphics (types 1 to 6): a separate VirtualAlloc region of ordinary CPU memory;
//! - physical (types 7 to 9): committed host pages (the title uses them as mapped RAM); an
//!   alignment beyond the allocation granularity is refused and logged.

use std::alloc::Layout;
use std::ffi::c_void;

use victory_hle::modules::xmem::EraFields;
use windows_sys::Win32::Foundation::{
    ERROR_INVALID_PARAMETER, ERROR_NOT_ENOUGH_MEMORY, ERROR_NOT_SUPPORTED, GetLastError,
};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc};

use crate::log::log;
use crate::pages::{allocate_aligned, geometry, release};

const GRAPHICS_FIRST: u32 = 1;
const GRAPHICS_LAST: u32 = 6;
const PHYSICAL_LAST: u32 = 9;
const UNDEFINED_PAGE_SIZE: u32 = 3;
/// The PC's policy, stricter than ERA: a cache line, and a page for big blocks.
const SMALL_ALIGNMENT: usize = 64;
const PAGE_ALIGNMENT: usize = 4096;
/// Bytes of the ERA page sizes 0 to 2 (4 KiB, 64 KiB, 4 MiB).
const PAGE_BYTES: [usize; 3] = [4 << 10, 64 << 10, 4 << 20];

/// How a block goes back: Rust's allocator (with its layout) or VirtualFree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    Heap(Layout),
    Pages,
    Graphics,
}

/// A served block, or the Win32 error the caller sets.
pub(crate) type Served = Result<(*mut c_void, Placement), u32>;

fn or_no_memory(block: *mut c_void, placement: Placement) -> Served {
    if block.is_null() {
        Err(ERROR_NOT_ENOUGH_MEMORY)
    } else {
        Ok((block, placement))
    }
}

/// Beyond a page of alignment: an own reservation whose useful pages are committed (VirtualAlloc
/// returns them zeroed).
fn allocate_pages(bytes: usize, alignment: usize) -> *mut c_void {
    let page = geometry().page;
    let committed = (bytes + page - 1) & !(page - 1);
    allocate_aligned(
        committed,
        alignment,
        MEM_RESERVE | MEM_COMMIT,
        PAGE_READWRITE,
    )
}

fn allocate_heap(bytes: usize, requested: usize) -> Served {
    let floor = if bytes >= PAGE_ALIGNMENT {
        PAGE_ALIGNMENT
    } else {
        SMALL_ALIGNMENT
    };
    let alignment = requested.max(floor);
    if alignment > geometry().page {
        return or_no_memory(allocate_pages(bytes, alignment), Placement::Pages);
    }
    let layout = Layout::from_size_align(bytes, alignment).map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
    // SAFETY: the layout has a non-zero size (bytes >= 1).
    let block = unsafe { std::alloc::alloc_zeroed(layout) };
    or_no_memory(block.cast(), Placement::Heap(layout))
}

/// The title's physical memory: committed host pages, at the requested alignment while it fits
/// in the granularity (64 KiB), refused and logged beyond.
fn allocate_physical(bytes: usize, alignment: usize) -> Served {
    if alignment > geometry().granularity {
        log!(
            "[xmem] XMemAlloc: physical memory, alignment {alignment} beyond the granularity not held"
        );
        return Err(ERROR_NOT_SUPPORTED);
    }
    or_no_memory(allocate_pages(bytes, alignment), Placement::Pages)
}

fn allocate_graphics(bytes: usize, fields: &EraFields, alignment: usize) -> Served {
    // A VirtualAlloc region starts on the allocation granularity (64 KiB on x64), which covers
    // 4 and 64 KiB pages. 4 MiB pages and larger alignments are not held.
    let page = PAGE_BYTES[fields.page_size as usize];
    if page.max(alignment) > geometry().granularity {
        log!(
            "[xmem] XMemAlloc: graphics type {}, page {page}, alignment {alignment} not held on PC",
            fields.memory_type
        );
        return Err(ERROR_NOT_SUPPORTED);
    }
    // SAFETY: a new region at an address of the system's choice.
    let block = unsafe {
        VirtualAlloc(
            std::ptr::null(),
            bytes,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        )
    };
    if block.is_null() {
        // SAFETY: plain thread-local state.
        return Err(unsafe { GetLastError() });
    }
    Ok((block, Placement::Graphics))
}

/// A zero-filled block, or the error to set: ERROR_INVALID_PARAMETER for an undefined field,
/// ERROR_NOT_SUPPORTED for alignments the PC cannot hold, ERROR_NOT_ENOUGH_MEMORY for an
/// oversized request. A size of zero gets a one-byte block.
pub(crate) fn allocate(bytes: usize, attributes: u64) -> Served {
    let fields = EraFields::decode(attributes);
    // Object type and allocator id are labels: SlimEra's default heap ignores them.
    if fields.reserved != 0
        || fields.page_size == UNDEFINED_PAGE_SIZE
        || fields.memory_type > PHYSICAL_LAST
    {
        log!("[xmem] XMemAlloc: attributes {attributes:#x} undefined");
        return Err(ERROR_INVALID_PARAMETER);
    }
    let bytes = bytes.max(1);
    let alignment = fields.alignment_bytes();
    let limit = isize::MAX as usize;
    if bytes > limit - alignment - size_of::<usize>() {
        return Err(ERROR_NOT_ENOUGH_MEMORY);
    }
    match fields.memory_type {
        0 => allocate_heap(bytes, alignment),
        GRAPHICS_FIRST..=GRAPHICS_LAST => allocate_graphics(bytes, &fields, alignment),
        _ => allocate_physical(bytes, alignment),
    }
}

/// Gives a block back to the allocator that made it.
pub(crate) fn free(address: *mut c_void, placement: Placement) {
    match placement {
        // SAFETY: the block came from `alloc_zeroed` with this layout.
        Placement::Heap(layout) => unsafe { std::alloc::dealloc(address.cast(), layout) },
        Placement::Pages | Placement::Graphics => release(address),
    }
}
