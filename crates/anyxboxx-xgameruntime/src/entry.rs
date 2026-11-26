//! The functions the title imports from xgameruntime.dll (the initialisers, QueryApiImpl, the
//! error report). Each one is passed to Windows' runtime when it is installed, otherwise it is
//! answered here, as the C++ `xgameruntime.cpp` does.

use std::ffi::{CStr, c_char, c_void};
use std::sync::Mutex;

use crate::guid::Guid;
use crate::log::{self, trace};
use crate::object::{E_NOINTERFACE, HResult, S_OK};
use crate::{served, system, tables};

/// Undocumented arity: eight pointers cover the four register and four stack arguments.
type InitializeRoutine = unsafe extern "system" fn(
    *mut c_void,
    *mut c_void,
    *mut c_void,
    *mut c_void,
    *mut c_void,
    *mut c_void,
    *mut c_void,
    *mut c_void,
) -> HResult;
type QueryRoutine =
    unsafe extern "system" fn(*const Guid, *const Guid, *mut *mut c_void, *mut c_void) -> HResult;

/// Console device type of the title (`XSystemDeviceType::XboxScarlettAnaconda`).
pub const XBOX_SCARLETT_ANACONDA: i32 = 7;

/// Passes one of the initialisers to Windows' runtime, or answers `S_OK` here.
pub fn initialize(name: &CStr, args: [*mut c_void; 8]) -> HResult {
    let [a, b, c, d, e, f, g, h] = args;
    match system::function::<InitializeRoutine>(name) {
        Some(routine) => {
            // SAFETY: the export of Windows' runtime takes these eight words (see InitializeRoutine).
            let result = unsafe { routine(a, b, c, d, e, f, g, h) };
            trace!(
                "[runtime] {} -> 0x{:08X} (Windows)",
                name.to_string_lossy(),
                result as u32
            );
            result
        }
        None => {
            trace!(
                "[runtime] {} -> 0x{:08X}",
                name.to_string_lossy(),
                S_OK as u32
            );
            S_OK
        }
    }
}

/// Serves `(provider, iid)` from AnyXboxX, or delegates the query to Windows' runtime.
///
/// # Safety
/// `provider`, `iid` and `out` are null or valid pointers, as the title passes them;
/// `reserved` is passed on untouched.
pub unsafe fn query(
    provider: *const Guid,
    iid: *const Guid,
    out: *mut *mut c_void,
    reserved: *mut c_void,
) -> HResult {
    if !out.is_null() {
        // SAFETY: non-null, one pointer-sized slot, per this function's contract.
        unsafe { *out = std::ptr::null_mut() };
    }
    // SAFETY: null or valid GUID pointers, per this function's contract.
    let (Some(&provider_id), Some(&iid_id)) =
        (unsafe { provider.as_ref() }, unsafe { iid.as_ref() })
    else {
        return E_NOINTERFACE;
    };
    if out.is_null() {
        return E_NOINTERFACE;
    }
    // One runtime for everything: the Windows APIs take XAsyncBlocks and task queues that only
    // they can read when they created them. AnyXboxX serves its interfaces only when it runs alone.
    if !system::delegated()
        && let Some(object) = served::find(&provider_id, &iid_id)
    {
        // SAFETY: `out` was checked non-null.
        unsafe { *out = object.cast_mut() };
        trace_query(&provider_id, &iid_id, "servie par AnyXboxX");
        return S_OK;
    }
    let result = match system::function::<QueryRoutine>(c"QueryApiImpl") {
        // SAFETY: the pointers are the ones the title gave us, forwarded unchanged.
        Some(routine) => unsafe { routine(provider, iid, out, reserved) },
        None => E_NOINTERFACE,
    };
    let outcome = if system::delegated() {
        "déléguée à Windows"
    } else {
        "non servie"
    };
    trace_query(
        &provider_id,
        &iid_id,
        &format!("0x{:08X} ({outcome})", result as u32),
    );
    result
}

/// Names the API behind a (provider, interface) pair, for the trace.
fn describe(provider: &Guid, iid: &Guid) -> String {
    let apis: Vec<_> = tables::gdk()
        .into_iter()
        .filter(|api| api.provider == *provider && api.iid == *iid)
        .collect();
    match apis.first() {
        None => "interface inconnue".to_string(),
        Some(first) => format!("{}… ({} fonctions)", first.function, apis.len()),
    }
}

/// Traces the first query of each pair only: the task queue is asked thousands of times a second.
fn trace_query(provider: &Guid, iid: &Guid, outcome: &str) {
    static SEEN: Mutex<Vec<(Guid, Guid)>> = Mutex::new(Vec::new());
    const MAX_REMEMBERED: usize = 128;
    if !log::enabled() {
        return;
    }
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let key = (*provider, *iid);
    if seen.contains(&key) || seen.len() >= MAX_REMEMBERED {
        return;
    }
    seen.push(key);
    drop(seen);
    log::write(format_args!(
        "[runtime] QueryApiImpl {provider} {iid} : {} -> {outcome}",
        describe(provider, iid)
    ));
}

/// `XErrorReport`: the title reports an error to the runtime; always written to the journal.
///
/// # Safety
/// `message` is null or a NUL-terminated string.
pub unsafe fn error_report(code: HResult, message: *const c_char) {
    let message = if message.is_null() {
        String::new()
    } else {
        // SAFETY: non-null and NUL-terminated, per this function's contract.
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    };
    log::write(format_args!(
        "[XErrorReport] 0x{:08X} {message}",
        code as u32
    ));
}

/// The device type the title sees (the console's, not the PC's).
pub fn device_type() -> i32 {
    XBOX_SCARLETT_ANACONDA
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_type_is_the_scarlett_console() {
        assert_eq!(device_type(), 7);
    }

    #[test]
    fn missing_arguments_are_refused_with_e_nointerface_and_a_null_output() {
        let guid = Guid::parse("00000000-0000-0000-0000-000000000002").unwrap();
        let mut out: *mut c_void = std::ptr::dangling_mut::<c_void>();
        // SAFETY: valid GUID pointer; a null output pointer is allowed by the contract.
        let hr = unsafe { query(&guid, &guid, std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(hr, E_NOINTERFACE);
        // SAFETY: valid GUID and output pointers; a null provider is refused before any lookup.
        let hr = unsafe { query(std::ptr::null(), &guid, &mut out, std::ptr::null_mut()) };
        assert_eq!((hr, out), (E_NOINTERFACE, std::ptr::null_mut()));
    }
}
