//! Win32 functions imported by the manually mapped image.

use crate::redirect::{Redirect, redirect, runtime_for};
use std::ffi::{CStr, CString, c_void};
use std::ptr;
use std::sync::OnceLock;

type Handle = *mut c_void;
const CP_ACP: u32 = 0;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryExW(name: *const u16, file: Handle, flags: u32) -> Handle;
    fn LoadLibraryExA(name: *const u8, file: Handle, flags: u32) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> Handle;
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn GetModuleHandleA(name: *const u8) -> Handle;
    fn GetModuleFileNameW(module: Handle, buffer: *mut u16, size: u32) -> u32;
    fn GetModuleFileNameA(module: Handle, buffer: *mut u8, size: u32) -> u32;
    fn MultiByteToWideChar(
        code: u32,
        flags: u32,
        src: *const u8,
        len: i32,
        dst: *mut u16,
        cap: i32,
    ) -> i32;
    fn WideCharToMultiByte(
        code: u32,
        flags: u32,
        src: *const u16,
        len: i32,
        dst: *mut u8,
        cap: i32,
        default: *const u8,
        used: *mut i32,
    ) -> i32;
}

/// Values remain valid until process exit, as the intercepted image can create threads.
pub struct HookContext {
    pub base: usize,
    pub path: Vec<u16>,
    pub runtime: [usize; 6],
    pub measuring: bool,
}

static CONTEXT: OnceLock<HookContext> = OnceLock::new();

pub fn install(context: HookContext) -> Result<(), HookContext> {
    CONTEXT.set(context)
}

unsafe fn wide_string(name: *const u16) -> String {
    let mut len = 0;
    // SAFETY: callers pass Win32 NUL-terminated names.
    unsafe {
        while *name.add(len) != 0 {
            len += 1;
        }
    }
    // SAFETY: the loop found the terminator in the input string.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) })
}

unsafe fn ansi_string(name: *const u8) -> String {
    // SAFETY: callers pass Win32 NUL-terminated names, as required by these APIs.
    let length = unsafe { MultiByteToWideChar(CP_ACP, 0, name, -1, ptr::null_mut(), 0) };
    if length <= 1 {
        return String::new();
    }
    let mut buffer = vec![0; length as usize];
    // SAFETY: the output buffer has exactly the size requested by the conversion function.
    unsafe {
        MultiByteToWideChar(CP_ACP, 0, name, -1, buffer.as_mut_ptr(), length);
    }
    String::from_utf16_lossy(&buffer[..buffer.len() - 1])
}

fn runtime(name: &str) -> Option<Handle> {
    let context = CONTEXT.get()?;
    let handle = context.runtime[runtime_for(name)?];
    (handle != 0).then_some(handle as Handle)
}

unsafe extern "system" fn load_ex_w(name: *const u16, file: Handle, flags: u32) -> Handle {
    if !name.is_null() {
        // SAFETY: this shim has the same string contract as LoadLibraryExW.
        if let Some(module) = runtime(&unsafe { wide_string(name) }) {
            return module;
        }
    }
    // SAFETY: arguments are forwarded unchanged to Windows.
    unsafe { LoadLibraryExW(name, file, flags) }
}
unsafe extern "system" fn load_w(name: *const u16) -> Handle {
    // SAFETY: forwards the same string contract to the extended shim.
    unsafe { load_ex_w(name, ptr::null_mut(), 0) }
}
unsafe extern "system" fn load_ex_a(name: *const u8, file: Handle, flags: u32) -> Handle {
    if !name.is_null() {
        // SAFETY: this shim has the same string contract as LoadLibraryExA.
        if let Some(module) = runtime(&unsafe { ansi_string(name) }) {
            return module;
        }
    }
    // SAFETY: arguments are forwarded unchanged to Windows.
    unsafe { LoadLibraryExA(name, file, flags) }
}
unsafe extern "system" fn load_a(name: *const u8) -> Handle {
    // SAFETY: forwards the same string contract to the extended shim.
    unsafe { load_ex_a(name, ptr::null_mut(), 0) }
}

unsafe extern "system" fn proc_address(module: Handle, name: *const u8) -> Handle {
    if let Some(context) = CONTEXT.get()
        && name as usize > 0xffff
        && context.measuring
    {
        // SAFETY: values above 0xffff are NUL-terminated names by the Win32 API contract.
        let symbol = unsafe { CStr::from_ptr(name.cast()) }.to_bytes();
        // SAFETY: constant NUL-terminated module name.
        let dxgi = unsafe { GetModuleHandleW([100, 120, 103, 105, 46, 100, 108, 108, 0].as_ptr()) };
        if module == dxgi
            && let Some(Redirect::RuntimeExport(export)) = redirect("dxgi.dll", symbol, false, true)
        {
            let export = CString::new(export).expect("fixed export and C string contain no NUL");
            // SAFETY: the runtime handle and NUL-terminated export name follow Win32 rules.
            return unsafe { GetProcAddress(context.runtime[0] as Handle, export.as_ptr().cast()) };
        }
    }
    // SAFETY: name or ordinal and module are forwarded unchanged.
    unsafe { GetProcAddress(module, name) }
}

unsafe extern "system" fn module_w(name: *const u16) -> Handle {
    if name.is_null() {
        return CONTEXT.get().map_or(ptr::null_mut(), |c| c.base as Handle);
    }
    // SAFETY: this shim has the same string contract as GetModuleHandleW.
    if let Some(module) = runtime(&unsafe { wide_string(name) }) {
        return module;
    }
    // SAFETY: argument forwarded unchanged.
    unsafe { GetModuleHandleW(name) }
}
unsafe extern "system" fn module_a(name: *const u8) -> Handle {
    if name.is_null() {
        return CONTEXT.get().map_or(ptr::null_mut(), |c| c.base as Handle);
    }
    // SAFETY: this shim has the same string contract as GetModuleHandleA.
    if let Some(module) = runtime(&unsafe { ansi_string(name) }) {
        return module;
    }
    // SAFETY: argument forwarded unchanged.
    unsafe { GetModuleHandleA(name) }
}

unsafe extern "system" fn filename_w(module: Handle, buffer: *mut u16, size: u32) -> u32 {
    let Some(context) = CONTEXT.get() else {
        return 0;
    };
    if !module.is_null() && module as usize != context.base {
        // SAFETY: arguments forwarded unchanged.
        return unsafe { GetModuleFileNameW(module, buffer, size) };
    }
    if buffer.is_null() || size == 0 {
        return 0;
    }
    let count = context.path.len().min(size as usize - 1);
    // SAFETY: caller guarantees buffer holds `size` UTF-16 units.
    unsafe {
        ptr::copy_nonoverlapping(context.path.as_ptr(), buffer, count);
        *buffer.add(count) = 0;
    }
    count as u32
}

unsafe extern "system" fn filename_a(module: Handle, buffer: *mut u8, size: u32) -> u32 {
    let Some(context) = CONTEXT.get() else {
        return 0;
    };
    if !module.is_null() && module as usize != context.base {
        // SAFETY: arguments forwarded unchanged.
        return unsafe { GetModuleFileNameA(module, buffer, size) };
    }
    if buffer.is_null() || size == 0 {
        return 0;
    }
    let path: Vec<u16> = context.path.iter().copied().chain([0]).collect();
    // SAFETY: the path is terminated and the caller owns the output buffer.
    let length = unsafe {
        WideCharToMultiByte(
            CP_ACP,
            0,
            path.as_ptr(),
            -1,
            buffer,
            size as i32,
            ptr::null(),
            ptr::null_mut(),
        )
    };
    if length > 0 { length as u32 - 1 } else { 0 }
}

/// Address of an implemented shim, or None for an unknown symbol.
pub fn shim(name: &[u8]) -> Option<usize> {
    Some(match name {
        b"LoadLibraryExW" => load_ex_w as *const () as usize,
        b"LoadLibraryW" => load_w as *const () as usize,
        b"LoadLibraryExA" => load_ex_a as *const () as usize,
        b"LoadLibraryA" => load_a as *const () as usize,
        b"GetProcAddress" => proc_address as *const () as usize,
        b"GetModuleHandleW" => module_w as *const () as usize,
        b"GetModuleHandleA" => module_a as *const () as usize,
        b"GetModuleFileNameW" => filename_w as *const () as usize,
        b"GetModuleFileNameA" => filename_a as *const () as usize,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_shims_use_image_path_and_runtime_handles() {
        let context = HookContext {
            base: 0x12340000,
            path: "C:\\synthetic.exe".encode_utf16().collect(),
            runtime: [0x7777, 0x8888, 0, 0, 0, 0],
            measuring: false,
        };
        assert!(install(context).is_ok());
        // SAFETY: each name and output buffer in this test is valid for the called Win32 shim.
        unsafe {
            assert_eq!(module_a(ptr::null()), 0x12340000usize as Handle);
            assert_eq!(module_a(c"XMEM".as_ptr().cast()), 0x8888usize as Handle);
            assert_eq!(
                load_a(c"xgameruntime140.dll".as_ptr().cast()),
                0x7777usize as Handle
            );
            let mut wide = [0u16; 5];
            assert_eq!(filename_w(ptr::null_mut(), wide.as_mut_ptr(), 5), 4);
            assert_eq!(&wide, &[67, 58, 92, 115, 0]);
            let mut ansi = [0u8; 64];
            let length = filename_a(ptr::null_mut(), ansi.as_mut_ptr(), ansi.len() as u32);
            assert_eq!(&ansi[..length as usize], b"C:\\synthetic.exe");
        }
        assert!(shim(b"GetProcAddress").is_some());
        assert!(shim(b"NoSuchShim").is_none());
    }
}
