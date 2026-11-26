//! Report the first non-informational exception before Windows continues searching.

use std::ffi::c_void;
use std::fmt::{self, Write};
use std::ptr;
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};

const STD_ERROR_HANDLE: u32 = -12i32 as u32;
const MODULE_FROM_ADDRESS: u32 = 4;
const MODULE_UNCHANGED_REFCOUNT: u32 = 2;
const MAX_PATH: usize = 260;

#[repr(C)]
struct ExceptionRecord {
    code: u32,
    flags: u32,
    record: *mut ExceptionRecord,
    address: *mut c_void,
}
#[repr(C)]
struct ExceptionPointers {
    record: *mut ExceptionRecord,
    context: *mut c_void,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn AddVectoredExceptionHandler(
        first: u32,
        handler: unsafe extern "system" fn(*mut ExceptionPointers) -> i32,
    ) -> *mut c_void;
    fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut *mut c_void) -> i32;
    fn GetModuleFileNameW(module: *mut c_void, buffer: *mut u16, size: u32) -> u32;
    fn GetStdHandle(kind: u32) -> *mut c_void;
    fn WriteFile(
        file: *mut c_void,
        buffer: *const u8,
        count: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
}

struct State {
    base: usize,
    size: usize,
    path: Vec<u16>,
}
static STATE: OnceLock<State> = OnceLock::new();
static REPORTED: AtomicBool = AtomicBool::new(false);

/// Install the process-lifetime handler before invoking TLS callbacks.
pub fn install(base: usize, size: usize, path: Vec<u16>) -> Result<(), &'static str> {
    STATE
        .set(State { base, size, path })
        .map_err(|_| "gestionnaire d'exceptions déjà installé")?;
    // SAFETY: callback has the vectored exception handler ABI and all state outlives the handler.
    if unsafe { AddVectoredExceptionHandler(1, handler) }.is_null() {
        return Err("installation du gestionnaire d'exceptions impossible");
    }
    Ok(())
}

struct Buffer {
    bytes: [u8; 2048],
    len: usize,
}
impl Buffer {
    fn new() -> Self {
        Self {
            bytes: [0; 2048],
            len: 0,
        }
    }
}
impl Write for Buffer {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let available = self.bytes.len() - self.len;
        let count = value.len().min(available);
        self.bytes[self.len..self.len + count].copy_from_slice(&value.as_bytes()[..count]);
        self.len += count;
        Ok(())
    }
}

fn format_report(code: u32, path: &[u16], rva: usize) -> Buffer {
    let start = path
        .iter()
        .rposition(|&c| c == b'\\' as u16 || c == b'/' as u16)
        .map_or(0, |at| at + 1);
    let mut result = Buffer::new();
    let _ = write!(result, "[anyxboxx] première exception 0x{code:08X} : ");
    for character in char::decode_utf16(path[start..].iter().copied()) {
        let _ = result.write_char(character.unwrap_or(char::REPLACEMENT_CHARACTER));
    }
    let _ = writeln!(result, "+0x{rva:X}");
    result
}

unsafe extern "system" fn handler(pointers: *mut ExceptionPointers) -> i32 {
    let Some(state) = STATE.get() else {
        return 0;
    };
    // SAFETY: Windows supplies a valid exception record to a vectored handler.
    let record = unsafe { &*(*pointers).record };
    if record.code >> 30 == 1 || REPORTED.swap(true, Ordering::SeqCst) {
        return 0;
    }
    let address = record.address as usize;
    let mut start = state.base;
    let mut module_path = [0u16; MAX_PATH];
    let path = if address.wrapping_sub(start) < state.size {
        state.path.as_slice()
    } else {
        let mut module = ptr::null_mut();
        // SAFETY: FROM_ADDRESS treats the second parameter as an address, not a string.
        if unsafe {
            GetModuleHandleExW(
                MODULE_FROM_ADDRESS | MODULE_UNCHANGED_REFCOUNT,
                address as *const u16,
                &mut module,
            )
        } != 0
        {
            // SAFETY: buffer and module are valid for this Windows call.
            let length =
                unsafe { GetModuleFileNameW(module, module_path.as_mut_ptr(), MAX_PATH as u32) }
                    as usize;
            start = module as usize;
            &module_path[..length.min(MAX_PATH)]
        } else {
            start = 0;
            module_path[0] = b'?' as u16;
            &module_path[..1]
        }
    };
    let report = format_report(record.code, path, address.wrapping_sub(start));
    let mut written = 0;
    // SAFETY: the buffer is valid for `len` bytes, and the handle is the process stderr.
    unsafe {
        WriteFile(
            GetStdHandle(STD_ERROR_HANDLE),
            report.bytes.as_ptr(),
            report.len as u32,
            &mut written,
            ptr::null_mut(),
        );
    }
    0 // EXCEPTION_CONTINUE_SEARCH
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats_first_exception_with_basename_and_rva() {
        let path: Vec<_> = r"C:\sample\trap.exe".encode_utf16().collect();
        let report = format_report(0x80000003, &path, 0x1005);
        assert_eq!(
            std::str::from_utf8(&report.bytes[..report.len]).unwrap(),
            "[anyxboxx] première exception 0x80000003 : trap.exe+0x1005\n"
        );
    }
}
