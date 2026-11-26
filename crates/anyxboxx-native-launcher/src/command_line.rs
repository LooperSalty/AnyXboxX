//! Give the mapped image the same command line as the C++ native launcher.
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCommandLineW() -> *mut u16;
    fn GetCommandLineA() -> *mut u8;
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

fn replace<T: Copy + PartialEq + Default>(buffer: &mut [T], line: &[T]) -> bool {
    let available = buffer
        .iter()
        .position(|x| *x == T::default())
        .unwrap_or(buffer.len());
    if available < line.len() || buffer.len() <= line.len() {
        return false;
    }
    buffer[..line.len()].copy_from_slice(line);
    buffer[line.len()] = T::default();
    true
}

/// Rewrite Windows' process command-line buffers when the new text fits.
///
/// # Safety
/// This mutates process-wide Windows buffers. The caller must ensure no other thread reads them.
pub unsafe fn adopt(path: &Path) {
    let line: Vec<u16> = [b'"' as u16]
        .into_iter()
        .chain(path.as_os_str().encode_wide())
        .chain([b'"' as u16])
        .collect();
    // SAFETY: Windows returns process-lifetime, NUL-terminated command line buffers.
    let (wide, narrow) = unsafe { (GetCommandLineW(), GetCommandLineA()) };
    let mut wide_len = 0;
    // SAFETY: the returned buffer is NUL-terminated.
    unsafe {
        while *wide.add(wide_len) != 0 {
            wide_len += 1;
        }
    }
    // SAFETY: the caller guarantees no concurrent readers and the original buffer includes NUL.
    let buffer = unsafe { std::slice::from_raw_parts_mut(wide, wide_len + 1) };
    replace(buffer, &line);
    let mut ansi = vec![0u8; line.len().saturating_mul(2).saturating_add(1)];
    let terminated: Vec<u16> = line.into_iter().chain([0]).collect();
    // SAFETY: input is terminated and output has the indicated capacity.
    let count = unsafe {
        WideCharToMultiByte(
            0,
            0,
            terminated.as_ptr(),
            -1,
            ansi.as_mut_ptr(),
            ansi.len() as i32,
            ptr::null(),
            ptr::null_mut(),
        )
    };
    if count <= 0 {
        return;
    }
    let mut narrow_len = 0;
    // SAFETY: Windows returns a NUL-terminated ANSI buffer.
    unsafe {
        while *narrow.add(narrow_len) != 0 {
            narrow_len += 1;
        }
    }
    // SAFETY: the caller guarantees no concurrent readers and the original buffer includes NUL.
    let buffer = unsafe { std::slice::from_raw_parts_mut(narrow, narrow_len + 1) };
    replace(buffer, &ansi[..count as usize - 1]);
}

#[cfg(test)]
mod tests {
    use super::replace;
    #[test]
    fn replaces_only_if_original_command_line_is_long_enough() {
        let mut buffer = *b"launcher.exe arg\0";
        assert!(replace(&mut buffer, b"\"game.exe\""));
        assert_eq!(&buffer[..11], b"\"game.exe\"\0");
        let mut short = *b"x\0";
        assert!(!replace(&mut short, b"long"));
        assert_eq!(&short, b"x\0");
    }
}
