//! Unit tests of the xmem.dll port. The same behaviour is checked through the built DLL by the
//! C++ DLL's pytest files (`tests/test_rust_dlls.py`). Only the hook test calls `XMemAlloc`
//! (the hooks are process-wide and tests run in parallel); the others use `XMemAllocDefault`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::{
    ERROR_INVALID_PARAMETER, ERROR_NOACCESS, ERROR_NOT_ENOUGH_MEMORY, ERROR_NOT_SUPPORTED,
    GetLastError, SetLastError,
};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_FREE, MEM_RELEASE, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_READWRITE,
    VirtualFree, VirtualQuery,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;
use windows_sys::core::BOOL;

use super::*;

const BINK_WORD: u64 = 0x05e5_0000;
const AMD_WORD: u64 = 0xa800_4000;
const GRANULARITY: usize = 64 << 10;

/// The tests share one address space: a region freed by one test can be given to another running at
/// the same time, which made `graphics_blocks_get_their_own_region_flagged_by_the_query` fail now and
/// then (its freed block still answered MEM_COMMIT). Each test that allocates or queries takes turns.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn era_word(memory_type: u64, alignment_log2: u64, page_size: u64, reserved: u64) -> u64 {
    page_size << 14 | alignment_log2 << 24 | memory_type << 29 | reserved << 33
}

fn last_error() -> u32 {
    // SAFETY: plain thread-local state.
    unsafe { GetLastError() }
}

fn clear_error() {
    // SAFETY: plain thread-local state.
    unsafe { SetLastError(0) };
}

fn windows_query(address: *const c_void) -> MEMORY_BASIC_INFORMATION {
    // SAFETY: plain data, filled by VirtualQuery.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let size = size_of::<MEMORY_BASIC_INFORMATION>();
    // SAFETY: a sized local.
    assert_eq!(unsafe { VirtualQuery(address, &mut info, size) }, size);
    info
}

/// XMemVirtualQuery into a 64-byte buffer whose bytes after +0x30 are a sentinel.
fn xmem_query(address: *const c_void) -> (BOOL, [u8; 64]) {
    let mut buffer = [0xa5u8; 64];
    // SAFETY: the pseudo handle of our process and a 64-byte buffer.
    let ok = unsafe { XMemVirtualQuery(GetCurrentProcess(), address, buffer.as_mut_ptr().cast()) };
    (ok, buffer)
}

fn zeroed(block: *const c_void, len: usize) -> bool {
    // SAFETY: the caller's block holds `len` readable bytes.
    unsafe { std::slice::from_raw_parts(block.cast::<u8>(), len) }
        .iter()
        .all(|&byte| byte == 0)
}

#[test]
fn decode_export_splits_with_the_era_layout() {
    let _serial = serial();
    let mut fields = EraFields::default();
    // SAFETY: a writable structure.
    unsafe { AnyXboxX_DecodeAllocAttributes(AMD_WORD, &mut fields) };
    assert_eq!(
        (
            fields.memory_type,
            fields.page_size,
            fields.allocator_id,
            fields.alignment
        ),
        (5, 1, 0, 8)
    );
    // SAFETY: null is part of the contract.
    unsafe { AnyXboxX_DecodeAllocAttributes(AMD_WORD, std::ptr::null_mut()) };
}

#[test]
fn heap_blocks_honour_the_alignment_and_are_zeroed() {
    let _serial = serial();
    for log2 in [0, 5, 12, 16, 20] {
        let word = era_word(0, log2, 0, 0) | 0xe5 << 16;
        let block = XMemAllocDefault(257, word);
        assert!(!block.is_null());
        assert_eq!(block as usize % (1 << log2.max(6)), 0, "log2 {log2}");
        assert!(zeroed(block, 257));
        // SAFETY: 257 bytes were allocated.
        unsafe { block.cast::<u8>().write_bytes(0x51, 257) };
        XMemFreeDefault(block, word);
    }
    let large = XMemAllocDefault(8192, 0);
    assert_eq!(large as usize % 4096, 0);
    XMemFreeDefault(large, 0);
}

#[test]
fn graphics_blocks_get_their_own_region_flagged_by_the_query() {
    let _serial = serial();
    let graphics = XMemAllocDefault(4096, AMD_WORD);
    let heap = XMemAllocDefault(257, BINK_WORD);
    assert_eq!(graphics as usize % GRANULARITY, 0);
    let info = windows_query(graphics);
    assert_eq!(
        (info.AllocationBase, info.State, info.Protect),
        (graphics, MEM_COMMIT, PAGE_READWRITE)
    );
    // SAFETY: inside the 4096-byte block.
    let (ok, buffer) = xmem_query(unsafe { graphics.cast::<u8>().add(100) }.cast());
    assert_eq!((ok, buffer[0x30]), (1, 1));
    assert!(buffer[0x31..].iter().all(|&byte| byte == 0xa5));
    assert_eq!(xmem_query(heap).1[0x30], 0);
    // Freed with other attributes: the block still goes back to its allocator.
    XMemFree(graphics, 0);
    XMemFree(heap, 0);
    assert_eq!(windows_query(graphics).State, MEM_FREE);
    let (ok, buffer) = xmem_query(graphics);
    assert_eq!((ok, buffer[0x30]), (1, 0));
}

#[test]
fn physical_type_is_served_as_committed_pages() {
    let _serial = serial();
    let word = 0xf081_4000; // type 7, 64 KiB pages, 64 KiB alignment
    let block = XMemAllocDefault(0x6_0000, word);
    assert!(!block.is_null() && (block as usize).is_multiple_of(GRANULARITY));
    assert!(zeroed(block, 0x6_0000));
    XMemFreeDefault(block, word);
    assert_eq!(windows_query(block).State, MEM_FREE);
}

#[test]
fn unservable_attributes_return_null_with_an_error() {
    let _serial = serial();
    for (word, error) in [
        (era_word(7, 17, 0, 0), ERROR_NOT_SUPPORTED),
        (era_word(12, 0, 0, 0), ERROR_INVALID_PARAMETER),
        (era_word(0, 0, 3, 0), ERROR_INVALID_PARAMETER),
        (BINK_WORD | 1 << 40, ERROR_INVALID_PARAMETER),
        (era_word(5, 0, 2, 0), ERROR_NOT_SUPPORTED),
        (era_word(5, 17, 0, 0), ERROR_NOT_SUPPORTED),
    ] {
        clear_error();
        assert!(XMemAllocDefault(4096, word).is_null(), "{word:#x}");
        assert_eq!(last_error(), error, "{word:#x}");
    }
}

#[test]
fn query_fails_cleanly() {
    let _serial = serial();
    let mut buffer = [0xd7u8; 64];
    // SAFETY: a 64-byte buffer; the address is in kernel space.
    let ok = unsafe {
        XMemVirtualQuery(
            GetCurrentProcess(),
            0xffff_8000_0000_0000_usize as *const c_void,
            buffer.as_mut_ptr().cast(),
        )
    };
    assert_eq!(ok, 0);
    assert_eq!(buffer, [0xd7; 64]);
    clear_error();
    // SAFETY: a null buffer is refused before any write.
    let ok = unsafe {
        XMemVirtualQuery(
            GetCurrentProcess(),
            buffer.as_ptr().cast(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!((ok, last_error()), (0, ERROR_NOACCESS));
}

static HOOKED: AtomicUsize = AtomicUsize::new(0);
static mut HOOK_BLOCK: [u8; 64] = [0; 64];

unsafe extern "system" fn hook_alloc(_size: usize, _attributes: u64) -> *mut c_void {
    (&raw mut HOOK_BLOCK).cast()
}

unsafe extern "system" fn hook_free(address: *mut c_void, _attributes: u64) {
    HOOKED.store(address as usize, Ordering::SeqCst);
}

#[test]
fn each_block_goes_back_to_the_allocator_that_made_it() {
    let _serial = serial();
    let before = XMemAlloc(64, 0);
    XMemSetAllocationHooks(Some(hook_alloc), Some(hook_free));
    let (mut alloc, mut release) = (None, None);
    // SAFETY: two writable slots.
    unsafe { XMemGetAllocationHooks(&mut alloc, &mut release) };
    assert!(alloc.is_some() && release.is_some());
    let hooked = XMemAlloc(32, 0);
    assert_eq!(hooked, (&raw mut HOOK_BLOCK).cast());
    XMemFree(before, 0); // a default block: freed by us
    assert_eq!(HOOKED.load(Ordering::SeqCst), 0);
    XMemFree(hooked, 0); // the title's block: back to its hook
    assert_eq!(HOOKED.load(Ordering::SeqCst), hooked as usize);
    XMemSetAllocationHooks(None, None);
    // SAFETY: two writable slots.
    unsafe { XMemGetAllocationHooks(&mut alloc, &mut release) };
    assert!(alloc.is_none() && release.is_none());
}

fn release(address: *mut c_void) {
    // SAFETY: a reservation made by the test.
    unsafe { VirtualFree(address, 0, MEM_RELEASE) };
}

#[test]
fn virtual_alloc_serves_the_measured_shapes() {
    let _serial = serial();
    for (kind, attributes, protect, state) in [
        (0x2040_1000, 0x283, 0x42404, MEM_COMMIT),
        (0x2040_2000, 0x21, 4, MEM_RESERVE),
        (0x2040_6000, 0x27, 0x1004, MEM_RESERVE),
        (0x2000_3000, 0x183, 0x22004, MEM_COMMIT),
        (0x4_2000, 7, 1, MEM_RESERVE),
    ] {
        let at = XMemVirtualAlloc(std::ptr::null_mut(), 0x1_0000, kind, attributes, protect);
        assert!(!at.is_null(), "{kind:#x}");
        assert_eq!(windows_query(at).State, state, "{kind:#x}");
        release(at);
    }
    let reserved = XMemVirtualAlloc(std::ptr::null_mut(), 0x1_0000, 0x2040_2000, 0x21, 4);
    let committed = XMemVirtualAlloc(reserved, 0x1_0000, 0x2040_1000, 0x283, 4);
    assert_eq!(committed, reserved);
    assert_eq!(windows_query(reserved).State, MEM_COMMIT);
    release(reserved);
}

#[test]
fn virtual_alloc_refuses_undefined_flags() {
    let _serial = serial();
    for (kind, attributes, protect) in [
        (0x4_2000, 7, 0),       // no protection
        (0x2040_0000, 0x21, 4), // neither reserve nor commit
        (0x4_2000, 1 << 33, 4), // reserved ERA bit
    ] {
        clear_error();
        let at = XMemVirtualAlloc(std::ptr::null_mut(), 0x1_0000, kind, attributes, protect);
        assert!(at.is_null());
        assert_eq!(last_error(), ERROR_INVALID_PARAMETER);
    }
}

#[test]
fn virtual_alloc_refuses_overflowing_sizes_without_panicking() {
    let _serial = serial();
    for address in [std::ptr::null_mut(), 0x1_0000_usize as *mut c_void] {
        clear_error();
        let at = XMemVirtualAlloc(address, usize::MAX, 0x2040_2000, 0x21, 4);
        assert!(at.is_null());
        assert_eq!(last_error(), ERROR_NOT_ENOUGH_MEMORY);
    }
}

#[test]
fn physical_pages_are_issued_mapped_and_freed() {
    let _serial = serial();
    let mut count = 2usize;
    let mut frames = [0usize; 2];
    // SAFETY: a count and room for two numbers.
    assert_eq!(
        unsafe { XMemAllocatePhysicalPages(0x280, &mut count, frames.as_mut_ptr()) },
        1
    );
    assert!(count == 2 && frames[0] != frames[1] && frames[0] >= 0x100);
    // SAFETY: two live numbers, mapped where the system likes.
    let mapped = unsafe { XMemMapPhysicalPagesScatter(std::ptr::null_mut(), 2, frames.as_ptr()) };
    assert!(!mapped.is_null());
    assert_eq!(windows_query(mapped).State, MEM_COMMIT);
    release(mapped);
    // SAFETY: two numbers.
    assert_eq!(unsafe { XMemFreePhysicalPages(2, frames.as_ptr()) }, 1);
    clear_error();
    // SAFETY: freed numbers are refused before any mapping.
    let refused = unsafe { XMemMapPhysicalPages(std::ptr::null_mut(), 2, frames.as_ptr()) };
    assert!(refused.is_null() && last_error() == ERROR_INVALID_PARAMETER);
    // SAFETY: as above.
    assert_eq!(unsafe { XMemFreePhysicalPages(2, frames.as_ptr()) }, 0);
}

#[test]
fn physical_requests_out_of_shape_are_refused() {
    let _serial = serial();
    let mut count = 0usize;
    let mut frames = [0usize; 1];
    clear_error();
    // SAFETY: a count and room for one number.
    let ok = unsafe { XMemAllocatePhysicalPages(0x280, &mut count, frames.as_mut_ptr()) };
    assert_eq!((ok, last_error()), (0, ERROR_INVALID_PARAMETER));
    count = 1;
    // SAFETY: as above; bit 33 is a reserved ERA bit.
    let ok = unsafe { XMemAllocatePhysicalPages(1 << 33, &mut count, frames.as_mut_ptr()) };
    assert_eq!(ok, 0);
}

#[test]
fn unserved_calls_say_so() {
    let _serial = serial();
    assert_eq!(XMemIsToolsMemAvailable(), 0);
    clear_error();
    assert_eq!(XMemTransferMemory(), 0);
    assert_eq!(last_error(), ERROR_NOT_SUPPORTED);
    clear_error();
    assert_eq!(XMemGetWorkingSetStatistics(0, std::ptr::null_mut()), 0);
    assert_eq!(last_error(), ERROR_NOT_SUPPORTED);
}
