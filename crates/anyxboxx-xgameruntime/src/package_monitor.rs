//! Monitors for the package that is already installed locally.
use crate::object::{E_INVALIDARG, HResult, S_OK};
use crate::save_async::{self, Completion};
use std::collections::HashSet;
use std::ffi::{c_char, c_void};
use std::mem::size_of;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use victory_hle::modules::xasync::AsyncBlock;

static INSTALL_IDENTITY: [u8; 1] = [11];
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(0);

pub unsafe extern "system" fn register(
    _this: *mut c_void,
    _owner: *mut c_void,
    _context: *mut c_void,
    _callback: *mut c_void,
    token: *mut u64,
) -> HResult {
    if token.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller supplies a writable token.
    unsafe {
        *token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    }
    S_OK
}

pub unsafe extern "system" fn unregister_progress(
    _this: *mut c_void,
    _monitor: *mut c_void,
    _token: u64,
    _wait: bool,
) -> bool {
    true
}

pub unsafe extern "system" fn unregister_package(
    _this: *mut c_void,
    _token: u64,
    _wait: bool,
) -> bool {
    true
}

fn identity() -> *const c_void {
    INSTALL_IDENTITY.as_ptr().cast()
}

fn monitors() -> &'static Mutex<HashSet<usize>> {
    static MONITORS: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
    MONITORS.get_or_init(|| Mutex::new(HashSet::new()))
}
pub unsafe extern "system" fn create(
    _this: *mut c_void,
    _id: *const c_char,
    _count: u32,
    _selectors: *mut c_void,
    _interval: u32,
    _queue: *mut c_void,
    output: *mut *mut c_void,
) -> HResult {
    if output.is_null() {
        return E_INVALIDARG;
    }
    let monitor = Box::into_raw(Box::new(0u8)).cast::<c_void>();
    monitors().lock().unwrap().insert(monitor as usize);
    // SAFETY: the caller provides a writable output pointer.
    unsafe {
        *output = monitor;
    }
    S_OK
}
pub unsafe extern "system" fn install(
    this: *mut c_void,
    id: *const c_char,
    count: u32,
    selectors: *mut c_void,
    interval: u32,
    _suppress_ui: bool,
    queue: *mut c_void,
    output: *mut *mut c_void,
) -> HResult {
    // SAFETY: arguments follow create's contract.
    unsafe { create(this, id, count, selectors, interval, queue, output) }
}

pub unsafe extern "system" fn install_async(
    _this: *mut c_void,
    _id: *const c_char,
    _count: u32,
    _selectors: *mut c_void,
    _interval: u32,
    _suppress_ui: bool,
    block: *mut AsyncBlock,
) -> HResult {
    let mut monitor = std::ptr::null_mut();
    // SAFETY: monitor is a writable local output, and other inputs are ignored.
    unsafe {
        create(
            std::ptr::null_mut(),
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut monitor,
        )
    };
    let mut completion = Completion::value(S_OK, monitor);
    completion.discard = Some(Box::new(move || {
        // SAFETY: the undelivered monitor is still owned by this completion.
        unsafe { close(std::ptr::null_mut(), monitor) };
    }));
    save_async::begin(block, identity(), completion)
}

pub unsafe extern "system" fn install_result(
    _this: *mut c_void,
    block: *mut AsyncBlock,
    output: *mut *mut c_void,
) -> HResult {
    save_async::result(block, identity(), size_of::<*mut c_void>(), output.cast())
}
pub unsafe extern "system" fn close(_this: *mut c_void, monitor: *mut c_void) {
    if monitors().lock().unwrap().remove(&(monitor as usize)) {
        // SAFETY: a registered monitor was allocated above and is removed only once.
        drop(unsafe { Box::from_raw(monitor.cast::<u8>()) });
    }
}
#[repr(C)]
pub struct InstallationProgress {
    total: u64,
    installed: u64,
    launch: u64,
    launchable: bool,
    completed: bool,
}
pub unsafe extern "system" fn progress(
    _this: *mut c_void,
    _monitor: *mut c_void,
    progress: *mut InstallationProgress,
) {
    if !progress.is_null() {
        // SAFETY: the caller supplies writable progress storage.
        unsafe {
            *progress = InstallationProgress {
                total: 1,
                installed: 1,
                launch: 1,
                launchable: true,
                completed: true,
            };
        }
    }
}
pub unsafe extern "system" fn update(_this: *mut c_void, _monitor: *mut c_void) -> bool {
    false
}
