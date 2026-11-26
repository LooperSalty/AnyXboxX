//! XPackage mounts map to the configured local game folder.
use crate::object::{E_INVALIDARG, E_NOT_SUFFICIENT_BUFFER, HResult, S_OK};
use crate::save_async::{self, Completion};
use crate::save_store::E_PATH_NOT_FOUND;
use std::collections::HashMap;
use std::ffi::{c_char, c_void};
use std::sync::{Mutex, OnceLock};
use victory_hle::modules::xasync::AsyncBlock;

static IDENTITY: [u8; 1] = [12];
fn mounts() -> &'static Mutex<HashMap<usize, Vec<u8>>> {
    static MOUNTS: OnceLock<Mutex<HashMap<usize, Vec<u8>>>> = OnceLock::new();
    MOUNTS.get_or_init(|| Mutex::new(HashMap::new()))
}
fn identity() -> *const c_void {
    IDENTITY.as_ptr().cast()
}

pub unsafe extern "system" fn mount_async(
    _this: *mut c_void,
    _id: *const c_char,
    block: *mut AsyncBlock,
) -> HResult {
    let path = std::env::var_os("ANYXBOXX_GAME_DIR")
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    if path.is_empty() {
        return save_async::begin(block, identity(), Completion::status(E_PATH_NOT_FOUND));
    }
    let mount = Box::into_raw(Box::new(0u8)).cast::<c_void>();
    mounts()
        .lock()
        .unwrap()
        .insert(mount as usize, path.into_bytes());
    let mut completion = Completion::value(S_OK, mount);
    completion.discard = Some(Box::new(move || {
        // SAFETY: an undelivered mount belongs to this completion.
        unsafe { close(std::ptr::null_mut(), mount) };
    }));
    save_async::begin(block, identity(), completion)
}
pub unsafe extern "system" fn mount_result(
    _this: *mut c_void,
    block: *mut AsyncBlock,
    output: *mut *mut c_void,
) -> HResult {
    save_async::result(
        block,
        identity(),
        std::mem::size_of::<*mut c_void>(),
        output.cast(),
    )
}
pub unsafe extern "system" fn path_size(
    _this: *mut c_void,
    mount: *mut c_void,
    size: *mut usize,
) -> HResult {
    if size.is_null() {
        return E_INVALIDARG;
    }
    let paths = mounts().lock().unwrap();
    let Some(path) = paths.get(&(mount as usize)) else {
        return E_INVALIDARG;
    };
    // SAFETY: the title provides writable size storage.
    unsafe {
        *size = path.len() + 1;
    }
    S_OK
}
pub unsafe extern "system" fn path(
    _this: *mut c_void,
    mount: *mut c_void,
    size: usize,
    output: *mut c_char,
) -> HResult {
    let paths = mounts().lock().unwrap();
    let Some(path) = paths.get(&(mount as usize)) else {
        return E_INVALIDARG;
    };
    if output.is_null() || size <= path.len() {
        return E_NOT_SUFFICIENT_BUFFER;
    }
    // SAFETY: the caller provides size writable bytes and we checked that they fit.
    unsafe {
        std::ptr::copy_nonoverlapping(path.as_ptr(), output.cast(), path.len());
        output.add(path.len()).write(0);
    }
    S_OK
}
pub unsafe extern "system" fn close(_this: *mut c_void, mount: *mut c_void) {
    if mounts().lock().unwrap().remove(&(mount as usize)).is_some() {
        // SAFETY: this registered allocation is removed only once.
        drop(unsafe { Box::from_raw(mount.cast::<u8>()) });
    }
}
