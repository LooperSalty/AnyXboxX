//! The GDK runtime of Windows (Gaming Services, `System32\xgameruntime.dll`), when it is installed.
//!
//! Loaded by its full path (a bare name would give back this DLL). When it is there, the title's
//! requests are passed to it: the game works, and the trace lists what would have to be written
//! here. `ANYXBOXX_SYSTEM_RUNTIME=0` switches the delegation off, as the tests do.

use std::ffi::CStr;
use std::sync::OnceLock;

use crate::log::trace;

const RUNTIME_FILE: &str = "\\xgameruntime.dll";

/// The module handle of Windows' runtime, `None` when absent or switched off.
fn module() -> Option<usize> {
    static MODULE: OnceLock<Option<usize>> = OnceLock::new();
    *MODULE.get_or_init(load)
}

#[cfg(windows)]
fn load() -> Option<usize> {
    use windows_sys::Win32::Foundation::MAX_PATH;
    use windows_sys::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

    if std::env::var("ANYXBOXX_SYSTEM_RUNTIME").is_ok_and(|value| value == "0") {
        return None;
    }
    let mut directory = [0u16; MAX_PATH as usize];
    // SAFETY: the buffer holds MAX_PATH wide characters, as the call expects.
    let length = unsafe { GetSystemDirectoryW(directory.as_mut_ptr(), MAX_PATH) } as usize;
    // Room for the file name must remain inside MAX_PATH (the C++ runtime checks the same).
    if length == 0 || length > MAX_PATH as usize - 20 {
        return None;
    }
    let mut path = directory[..length].to_vec();
    path.extend(RUNTIME_FILE.encode_utf16());
    path.push(0);
    // SAFETY: `path` is NUL-terminated; no file handle is passed.
    let module = unsafe {
        LoadLibraryExW(
            path.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    trace!(
        "[runtime] runtime GDK de Windows {}",
        if module.is_null() {
            "absent"
        } else {
            "trouvé : délégation active"
        }
    );
    (!module.is_null()).then_some(module as usize)
}

#[cfg(not(windows))]
fn load() -> Option<usize> {
    None
}

/// Whether requests are delegated to Windows' runtime.
pub fn delegated() -> bool {
    module().is_some()
}

/// The function `name` of Windows' runtime, typed as `F` (a function pointer type), if delegated.
#[cfg(windows)]
pub fn function<F: Copy>(name: &CStr) -> Option<F> {
    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::System::LibraryLoader::GetProcAddress;

    let module = module()? as HMODULE;
    // SAFETY: `name` is NUL-terminated (a CStr); the module handle came from LoadLibraryExW.
    let proc = unsafe { GetProcAddress(module, name.as_ptr().cast()) }?;
    // SAFETY: the caller names the signature the export has; both are one pointer wide.
    Some(unsafe { std::mem::transmute_copy::<_, F>(&proc) })
}

#[cfg(not(windows))]
pub fn function<F: Copy>(_name: &CStr) -> Option<F> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switched_off_is_never_delegated() {
        // The test binary runs with the switch off in the test environment (see tests/conftest.py);
        // here we only check the value read by `load` when the variable is set to 0.
        // SAFETY: tests run this in their own process; no other thread reads the variable.
        unsafe { std::env::set_var("ANYXBOXX_SYSTEM_RUNTIME", "0") };
        assert!(load().is_none());
    }
}
