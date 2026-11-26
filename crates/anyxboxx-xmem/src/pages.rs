//! Page-aligned reservations beyond the page, shared by the heap, physical memory and
//! `XMemVirtualAlloc` (port of `runtime/xmem/aligned_pages.cpp`). VirtualAlloc only guarantees
//! the allocation granularity (64 KiB); a larger alignment is met by reserving a wider range,
//! releasing it and allocating again at the aligned address inside it.

use std::ffi::c_void;
use std::sync::LazyLock;

use windows_sys::Win32::Foundation::{ERROR_NOT_ENOUGH_MEMORY, SetLastError};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_NOACCESS, VirtualAlloc, VirtualFree,
};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};

/// Another thread may take the range between the release and the aligned allocation.
const PLACEMENT_ATTEMPTS: usize = 8;

/// The host's page size and allocation granularity, read once.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Geometry {
    pub page: usize,
    pub granularity: usize,
}

pub(crate) fn geometry() -> Geometry {
    static GEOMETRY: LazyLock<Geometry> = LazyLock::new(|| {
        // SAFETY: GetSystemInfo fills the structure it is given.
        let info = unsafe {
            let mut info: SYSTEM_INFO = std::mem::zeroed();
            GetSystemInfo(&mut info);
            info
        };
        Geometry {
            page: info.dwPageSize as usize,
            granularity: info.dwAllocationGranularity as usize,
        }
    });
    *GEOMETRY
}

/// Committing a free range must also reserve it: the reservation is made at the aligned address.
fn placement_type(kind: u32) -> u32 {
    if kind & MEM_COMMIT != 0 {
        kind | MEM_RESERVE
    } else {
        kind
    }
}

fn try_aligned(bytes: usize, alignment: usize, kind: u32, protect: u32) -> *mut c_void {
    // SAFETY: a fresh reservation of our own, released before the aligned allocation.
    unsafe {
        let probe = VirtualAlloc(
            std::ptr::null(),
            bytes + alignment,
            MEM_RESERVE,
            PAGE_NOACCESS,
        );
        if probe.is_null() {
            return probe;
        }
        let aligned = (probe as usize + alignment - 1) & !(alignment - 1);
        VirtualFree(probe, 0, MEM_RELEASE);
        VirtualAlloc(
            aligned as *const c_void,
            bytes,
            placement_type(kind),
            protect,
        )
    }
}

/// A block of `bytes` at a multiple of `alignment` (a power of two), or null with the last error
/// set. Up to a page of alignment it is a plain VirtualAlloc.
pub(crate) fn allocate_aligned(
    bytes: usize,
    alignment: usize,
    kind: u32,
    protect: u32,
) -> *mut c_void {
    if alignment <= geometry().page {
        // SAFETY: a new allocation at an address of the system's choice.
        return unsafe { VirtualAlloc(std::ptr::null(), bytes, kind, protect) };
    }
    if bytes > usize::MAX - alignment {
        // SAFETY: plain thread-local state.
        unsafe { SetLastError(ERROR_NOT_ENOUGH_MEMORY) };
        return std::ptr::null_mut();
    }
    for _ in 0..PLACEMENT_ATTEMPTS {
        let block = try_aligned(bytes, alignment, kind, protect);
        if !block.is_null() {
            return block;
        }
    }
    std::ptr::null_mut()
}

/// Releases a whole VirtualAlloc reservation by its base.
pub(crate) fn release(base: *mut c_void) {
    // SAFETY: the caller owns the reservation starting at `base`.
    unsafe { VirtualFree(base, 0, MEM_RELEASE) };
}
