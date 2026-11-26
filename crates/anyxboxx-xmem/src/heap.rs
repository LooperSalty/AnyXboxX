//! XMemAlloc, XMemFree, their defaults and hooks, XMemVirtualQuery and the statistics call
//! (port of `runtime/xmem/xmem.cpp`).

use std::collections::HashMap;
use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};

use victory_hle::modules::xmem::EraFields;
use windows_sys::Win32::Foundation::{
    ERROR_INVALID_ADDRESS, ERROR_NOT_SUPPORTED, HANDLE, SetLastError,
};
use windows_sys::core::BOOL;

use crate::era::{self, Placement};
use crate::era_query;
use crate::log::{log, trace};

pub type AllocRoutine = unsafe extern "system" fn(usize, u64) -> *mut c_void;
pub type FreeRoutine = unsafe extern "system" fn(*mut c_void, u64);

/// The query buffer of the BVH callers (seen by Xbox2Windows).
const QUERY_BUFFER_BYTES: usize = 64;

/// The title's hook pair, published as one pointer.
struct Hooks {
    alloc: Option<AllocRoutine>,
    release: Option<FreeRoutine>,
}

static NO_HOOKS: Hooks = Hooks {
    alloc: None,
    release: None,
};
static HOOKS: AtomicPtr<Hooks> = AtomicPtr::new((&raw const NO_HOOKS).cast_mut());

fn hooks() -> &'static Hooks {
    // SAFETY: HOOKS only ever holds NO_HOOKS or a leaked pair, both live forever.
    unsafe { &*HOOKS.load(Ordering::Acquire) }
}

/// Default blocks and their placement, until freed.
static BLOCKS: LazyLock<Mutex<HashMap<usize, Placement>>> = LazyLock::new(Mutex::default);

fn blocks() -> MutexGuard<'static, HashMap<usize, Placement>> {
    // Each change is one call: a poisoned map is still consistent.
    BLOCKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn set_error(error: u32) {
    // SAFETY: plain thread-local state.
    unsafe { SetLastError(error) };
}

/// Frees a default block; false if `address` is not one.
fn release_default(address: *mut c_void) -> bool {
    let Some(placement) = blocks().remove(&(address as usize)) else {
        return false;
    };
    if placement == Placement::Graphics {
        era_query::unregister_graphics_region(address);
    }
    era::free(address, placement);
    true
}

#[unsafe(no_mangle)]
pub extern "system" fn XMemAllocDefault(size: usize, attributes: u64) -> *mut c_void {
    let (block, placement) = match era::allocate(size, attributes) {
        Ok(served) => served,
        Err(error) => {
            trace!("[xmem] XMemAlloc {size} bytes (attributes {attributes:#x}) -> NULL");
            set_error(error);
            return std::ptr::null_mut();
        }
    };
    if placement == Placement::Graphics && !era_query::register_graphics_region(block, size.max(1))
    {
        log!("[xmem] XMemAlloc: graphics region {block:p} already registered");
        era::free(block, placement);
        set_error(ERROR_INVALID_ADDRESS);
        return std::ptr::null_mut();
    }
    blocks().insert(block as usize, placement);
    trace!("[xmem] XMemAlloc {size} bytes (attributes {attributes:#x}) -> {block:p}");
    block
}

#[unsafe(no_mangle)]
pub extern "system" fn XMemFreeDefault(address: *mut c_void, _attributes: u64) {
    if !address.is_null() && !release_default(address) {
        log!("[xmem] XMemFreeDefault: unknown block {address:p}, ignored");
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn XMemAlloc(size: usize, attributes: u64) -> *mut c_void {
    match hooks().alloc {
        // SAFETY: the title's own routine of this type.
        Some(alloc) => unsafe { alloc(size, attributes) },
        None => XMemAllocDefault(size, attributes),
    }
}

// The title calls this through its import table, passing pointers the real API trusts (as
// VIctory's xmem module does).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "system" fn XMemFree(address: *mut c_void, attributes: u64) {
    // A default block goes back to us, even if hooks were set since.
    if address.is_null() || release_default(address) {
        return;
    }
    match hooks().release {
        // SAFETY: the title's own routine of this type.
        Some(release) => unsafe { release(address, attributes) },
        None => log!("[xmem] XMemFree: unknown block {address:p} (hooks removed?), ignored"),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn XMemSetAllocationHooks(
    alloc: Option<AllocRoutine>,
    release: Option<FreeRoutine>,
) {
    // Pairs are never freed: another thread may still read the old one (a few bytes per call).
    let hooks: *const Hooks = if alloc.is_none() && release.is_none() {
        &raw const NO_HOOKS
    } else {
        Box::leak(Box::new(Hooks { alloc, release }))
    };
    HOOKS.store(hooks.cast_mut(), Ordering::Release);
}

/// # Safety
/// Each pointer is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn XMemGetAllocationHooks(
    alloc: *mut Option<AllocRoutine>,
    release: *mut Option<FreeRoutine>,
) {
    let hooks = hooks();
    // SAFETY: per this function's contract.
    unsafe {
        if let Some(alloc) = alloc.as_mut() {
            *alloc = hooks.alloc;
        }
        if let Some(release) = release.as_mut() {
            *release = hooks.release;
        }
    }
}

/// MEMORY_BASIC_INFORMATION then byte +0x30 (bit 0: graphics memory), see `era_query`.
///
/// # Safety
/// `buffer` is null or points at 64 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn XMemVirtualQuery(
    process: HANDLE,
    address: *const c_void,
    buffer: *mut c_void,
) -> BOOL {
    // SAFETY: forwarded contract.
    let written = unsafe { era_query::query(process, address, buffer, QUERY_BUFFER_BYTES) };
    (written != 0).into()
}

/// XMEM_WORKING_SET_STATISTICS is not public: failure (FALSE), which the known callers read as
/// "used memory unknown".
#[unsafe(no_mangle)]
pub extern "system" fn XMemGetWorkingSetStatistics(kind: c_int, _statistics: *mut c_void) -> BOOL {
    trace!("[xmem] XMemGetWorkingSetStatistics({kind}) not available");
    set_error(ERROR_NOT_SUPPORTED);
    0
}

/// AnyXboxX's own tool (not a console function): the ERA fields of an attribute word.
///
/// # Safety
/// `fields` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AnyXboxX_DecodeAllocAttributes(
    attributes: u64,
    fields: *mut EraFields,
) {
    // SAFETY: per this function's contract.
    if let Some(fields) = unsafe { fields.as_mut() } {
        *fields = EraFields::decode(attributes);
    }
}
