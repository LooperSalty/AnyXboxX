//! The registry the title sees: the console's OEM key, whose `ConsoleMode` gives the device type.
//!
//! `xgameruntime.lib` computes XSystemGetDeviceType by reading `HKLM\Software\Microsoft\Windows
//! NT\CurrentVersion\OEM`, value `ConsoleMode`; 0x64 gives XboxScarlettAnaconda (read in the GDK
//! 2610 disassembly). The native mode points the title's registry imports at these functions. Only
//! that key is simulated; everything else goes to the real registry. Port of `registry.cpp`.

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, REG_DWORD, REG_SAM_FLAGS, RegCloseKey, RegOpenKeyExW,
    RegQueryValueExW,
};

use crate::log::trace;

const OEM_KEY: &str = "Software\\Microsoft\\Windows NT\\CurrentVersion\\OEM";
const CONSOLE_MODE: &str = "ConsoleMode";
const SCARLETT_ANACONDA_MODE: u32 = 0x64;
/// Fictitious handle of the simulated key, never passed to the real registry ("AXXS").
const OEM_HANDLE: HKEY = 0x4158_5853_usize as HKEY;
const MAXIMUM_ALLOWED: REG_SAM_FLAGS = 0x0200_0000;
/// LONG, the type of the registry exports.
type LStatus = i32;

/// Whether `wide` (NUL-terminated, may be null) equals `text` ignoring ASCII case.
///
/// # Safety
/// `wide` is null or points at a NUL-terminated UTF-16 string.
unsafe fn same_text(wide: *const u16, text: &str) -> bool {
    if wide.is_null() {
        return false;
    }
    let mut index = 0;
    let mut expected = text.encode_utf16();
    loop {
        // SAFETY: the string is NUL-terminated, so reads stop at its end.
        let unit = unsafe { *wide.add(index) };
        index += 1;
        match expected.next() {
            None => return unit == 0,
            Some(want) => {
                if unit == 0 || !unit_eq_ignore_ascii_case(unit, want) {
                    return false;
                }
            }
        }
    }
}

fn unit_eq_ignore_ascii_case(a: u16, b: u16) -> bool {
    a == b || (a < 128 && b < 128 && (a as u8).eq_ignore_ascii_case(&(b as u8)))
}

/// Whether `(root, path)` names the simulated OEM key; a leading backslash is allowed.
///
/// # Safety
/// `path` is null or a NUL-terminated UTF-16 string.
unsafe fn is_oem_key(root: HKEY, path: *const u16) -> bool {
    if root != HKEY_LOCAL_MACHINE || path.is_null() {
        return false;
    }
    // SAFETY: a non-null path is NUL-terminated, per this function's contract.
    let first = unsafe { *path };
    let start = if first == u16::from(b'\\') {
        // SAFETY: the first unit is a backslash, so the string has at least one more unit.
        unsafe { path.add(1) }
    } else {
        path
    };
    // SAFETY: `start` points into the same NUL-terminated string.
    unsafe { same_text(start, OEM_KEY) }
}

/// `AnyXboxX_RegOpenKeyExW`: the OEM key opens as the fictitious handle; other keys go to Windows.
///
/// # Safety
/// As `RegOpenKeyExW`: `path` is a NUL-terminated string and `result` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AnyXboxX_RegOpenKeyExW(
    root: HKEY,
    path: *const u16,
    options: u32,
    access: REG_SAM_FLAGS,
    result: *mut HKEY,
) -> LStatus {
    // SAFETY: `path` is checked by `is_oem_key`, per this function's contract.
    if !result.is_null() && unsafe { is_oem_key(root, path) } {
        trace!("[runtime] registre : clé OEM simulée (Xbox Series X)");
        // SAFETY: `result` was checked non-null.
        unsafe { *result = OEM_HANDLE };
        return ERROR_SUCCESS as LStatus;
    }
    // SAFETY: forwarded with the caller's arguments.
    unsafe { RegOpenKeyExW(root, path, options, access, result) as LStatus }
}

/// `AnyXboxX_RegOpenKeyW`: `AnyXboxX_RegOpenKeyExW` with no options and maximum access.
///
/// # Safety
/// As [`AnyXboxX_RegOpenKeyExW`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AnyXboxX_RegOpenKeyW(
    root: HKEY,
    path: *const u16,
    result: *mut HKEY,
) -> LStatus {
    // SAFETY: forwarded with the caller's arguments.
    unsafe { AnyXboxX_RegOpenKeyExW(root, path, 0, MAXIMUM_ALLOWED, result) }
}

/// Writes the simulated `ConsoleMode` into the caller's buffers, with the registry's own rules
/// for type, size and `ERROR_MORE_DATA`.
///
/// # Safety
/// `data` and `size` are null or valid, as for `RegQueryValueExW`.
unsafe fn console_mode(kind: *mut u32, data: *mut u8, size: *mut u32) -> LStatus {
    if !kind.is_null() {
        // SAFETY: non-null and writable, per this function's contract.
        unsafe { *kind = REG_DWORD };
    }
    if !data.is_null() {
        // SAFETY: a non-null size is readable; a null one means the buffer size is unknown.
        if size.is_null() || unsafe { *size } < 4 {
            if !size.is_null() {
                // SAFETY: non-null and writable.
                unsafe { *size = 4 };
            }
            return ERROR_MORE_DATA as LStatus;
        }
        // SAFETY: the buffer holds at least four bytes, as checked above.
        unsafe { data.cast::<u32>().write_unaligned(SCARLETT_ANACONDA_MODE) };
    }
    if !size.is_null() {
        // SAFETY: non-null and writable.
        unsafe { *size = 4 };
    }
    ERROR_SUCCESS as LStatus
}

/// `AnyXboxX_RegQueryValueExW`: `ConsoleMode` of the simulated key; other keys go to Windows.
///
/// # Safety
/// As `RegQueryValueExW`.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AnyXboxX_RegQueryValueExW(
    key: HKEY,
    name: *const u16,
    reserved: *mut u32,
    kind: *mut u32,
    data: *mut u8,
    size: *mut u32,
) -> LStatus {
    if key != OEM_HANDLE {
        // SAFETY: forwarded with the caller's arguments.
        return unsafe { RegQueryValueExW(key, name, reserved, kind, data, size) as LStatus };
    }
    // SAFETY: `name` is null or NUL-terminated, per this function's contract.
    if !unsafe { same_text(name, CONSOLE_MODE) } {
        return ERROR_FILE_NOT_FOUND as LStatus;
    }
    // SAFETY: the buffers are the caller's, per this function's contract.
    unsafe { console_mode(kind, data, size) }
}

/// `AnyXboxX_RegCloseKey`: closing the simulated key does nothing.
///
/// # Safety
/// As `RegCloseKey`.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AnyXboxX_RegCloseKey(key: HKEY) -> LStatus {
    if key == OEM_HANDLE {
        return ERROR_SUCCESS as LStatus;
    }
    // SAFETY: forwarded with the caller's handle.
    unsafe { RegCloseKey(key) as LStatus }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }

    #[test]
    fn the_oem_key_is_recognised_whatever_its_case_or_leading_backslash() {
        let oem = wide("\\Software\\Microsoft\\windows nt\\currentversion\\oem");
        // SAFETY: NUL-terminated buffers.
        assert!(unsafe { is_oem_key(HKEY_LOCAL_MACHINE, oem.as_ptr()) });
        let other = wide("Software\\Microsoft");
        // SAFETY: as above.
        assert!(!unsafe { is_oem_key(HKEY_LOCAL_MACHINE, other.as_ptr()) });
        // SAFETY: as above; another root never matches.
        assert!(!unsafe { is_oem_key(OEM_HANDLE, oem.as_ptr()) });
    }

    #[test]
    fn console_mode_is_written_with_the_registry_size_rules() {
        let mut kind = 0u32;
        let mut size = 4u32;
        let mut value = [0u8; 4];
        // SAFETY: valid buffers of the sizes given.
        let status = unsafe { console_mode(&mut kind, value.as_mut_ptr(), &mut size) };
        assert_eq!(
            (status, kind, size),
            (ERROR_SUCCESS as LStatus, REG_DWORD, 4)
        );
        assert_eq!(u32::from_le_bytes(value), SCARLETT_ANACONDA_MODE);
        let mut small = 2u32;
        // SAFETY: as above; the size is too small, so nothing is written.
        let status = unsafe { console_mode(std::ptr::null_mut(), value.as_mut_ptr(), &mut small) };
        assert_eq!((status, small), (ERROR_MORE_DATA as LStatus, 4));
    }
}
