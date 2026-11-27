//! Virtual drives over host folders: on the console, title storage is a
//! drive of its own, and titles build paths on that assumption.
//!
//! Every title import that takes a path is bound here. A path on a virtual
//! drive (`X:\..`, `X:/..`, `X:`, `\\?\X:\..`) is rewritten to the host
//! folder behind it and handed to the host's own function; any other path
//! goes to the host unchanged. A rewritten narrow path goes to the host's
//! wide twin, so host folders outside the ANSI code page work; never to our
//! wide wrapper, which would rewrite the host path a second time when its
//! drive letter is mounted too. Letters are chosen among those the host
//! does not use, so no host drive is hidden.
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::too_many_arguments)]

use std::ffi::{CStr, c_char, c_void};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use windows_sys::Win32::Globalization::{CP_ACP, MultiByteToWideChar};
use windows_sys::Win32::Storage::FileSystem::GetLogicalDrives;

use crate::{HleExport, HleModule, hle_export};

/// The host's own function: what the import would have been bound to.
macro_rules! host {
    ($dll:literal, $name:literal: $ty:ty) => {{
        static ADDRESS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        let address = *ADDRESS.get_or_init(|| {
            crate::host::function($dll, $name)
                .unwrap_or_else(|| panic!("the host lacks {}!{}", $dll, $name))
        });
        // SAFETY: the host exports this function with this signature.
        unsafe { std::mem::transmute::<usize, $ty>(address) }
    }};
}
pub(crate) use host;

/// Mounted drives: upper-case letter and host root (UTF-16, no trailing
/// separator, no NUL).
static DRIVES: Mutex<Vec<(u16, Vec<u16>)>> = Mutex::new(Vec::new());

fn drives() -> MutexGuard<'static, Vec<(u16, Vec<u16>)>> {
    DRIVES.lock().unwrap_or_else(|e| e.into_inner())
}

/// Gives the host folder `root` a drive letter (the same one if it already
/// has one): the highest letter neither the host nor another mount uses.
pub fn mount(root: &Path) -> Option<char> {
    let wide = root_wide(root);
    let mut drives = drives();
    if let Some(&(letter, _)) = drives.iter().find(|(_, r)| *r == wide) {
        return char::from_u32(letter.into());
    }
    // SAFETY: no arguments.
    let host = unsafe { GetLogicalDrives() };
    let letter = (b'D'..=b'Z').rev().find(|&l| {
        host >> (l - b'A') & 1 == 0 && !drives.iter().any(|(m, _)| *m == u16::from(l))
    })?;
    tracing::info!(drive = %char::from(letter), root = %root.display(), "virtual drive");
    drives.push((letter.into(), wide));
    Some(letter.into())
}

/// Gives the host folder `root` a drive letter the console fixes (titles
/// open their package as `G:\..` and temporary files as `T:\..`). A host
/// drive of that letter is hidden from the title.
pub fn mount_at(letter: char, root: &Path) {
    let letter = letter.to_ascii_uppercase();
    let Some(index) = u32::from(letter)
        .checked_sub(u32::from('A'))
        .filter(|i| *i < 26)
    else {
        return;
    };
    // SAFETY: no arguments.
    let hides_host_drive = unsafe { GetLogicalDrives() } >> index & 1 != 0;
    tracing::info!(drive = %letter, root = %root.display(), hides_host_drive, "virtual drive");
    let mut drives = drives();
    drives.retain(|(l, _)| u32::from(*l) != u32::from(letter));
    drives.push((letter as u16, root_wide(root)));
}

/// `root` as UTF-16, absolute, without trailing separators (a drive root
/// keeps its own). An empty root is the current directory, as it is to a
/// path joined to it: it is the title's folder when the title is run by its
/// file name alone (`victory run GTA6.exe`).
fn root_wide(root: &Path) -> Vec<u16> {
    let root = if root.as_os_str().is_empty() {
        Path::new(".")
    } else {
        root
    };
    let root = std::path::absolute(root).unwrap_or_else(|_| root.to_owned());
    let mut wide: Vec<u16> = root.as_os_str().encode_wide().collect();
    while wide.len() > 3 && wide.last().is_some_and(|c| SEPARATORS.contains(c)) {
        wide.pop();
    }
    wide
}

const SEPARATORS: [u16; 2] = [b'\\' as u16, b'/' as u16];
const VERBATIM: [u16; 4] = [b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];

/// Host paths from this length on take the `\\?\` form, which lifts the
/// host's MAX_PATH limit (248 characters for a directory, 260 for a file).
const LONG: usize = 248;

/// `root` and the title's `tail` as a `\\?\` path. That form turns off the
/// host's own clean-up, so `/` becomes `\` and `.` and `..` are resolved
/// here (`..` stops at the root).
fn verbatim(root: &[u16], tail: &[u16]) -> Vec<u16> {
    let backslash = u16::from(b'\\');
    let mut out = if root.starts_with(&VERBATIM) {
        root.to_vec()
    } else if let Some(unc) = root.strip_prefix(&[backslash; 2][..]) {
        let mut out = VERBATIM.to_vec();
        out.extend("UNC\\".encode_utf16());
        out.extend_from_slice(unc);
        out
    } else {
        [&VERBATIM[..], root].concat()
    };
    while out.last() == Some(&backslash) {
        out.pop();
    }
    let dot = u16::from(b'.');
    let mut parts: Vec<&[u16]> = Vec::new();
    for part in tail.split(|c| SEPARATORS.contains(c)) {
        match part {
            [] => {}
            [d] if *d == dot => {}
            [d, e] if [*d, *e] == [dot; 2] => drop(parts.pop()),
            _ => parts.push(part),
        }
    }
    for part in parts {
        out.push(backslash);
        out.extend_from_slice(part);
    }
    out
}

/// The host path, NUL-terminated, for `path` (without its NUL) if it is on
/// a virtual drive.
pub(crate) fn translate(path: &[u16]) -> Option<Vec<u16>> {
    let (prefix, rest) = match path.strip_prefix(&VERBATIM[..]) {
        Some(rest) => (&VERBATIM[..], rest),
        None => (&[][..], path),
    };
    let &[letter, colon, ..] = rest else {
        return None;
    };
    if colon != u16::from(b':') {
        return None;
    }
    let letter = u8::try_from(letter).ok().filter(u8::is_ascii_alphabetic)?;
    let letter = u16::from(letter.to_ascii_uppercase());
    let drives = drives();
    let (_, root) = drives.iter().find(|(l, _)| *l == letter)?;
    let tail = &rest[2..];
    if prefix.is_empty() && root.len() + tail.len() >= LONG {
        let mut out = verbatim(root, tail);
        out.push(0);
        return Some(out);
    }
    let mut out = prefix.to_vec();
    out.extend_from_slice(root);
    // `X:` and `X:name` are relative to the root.
    if !tail.first().is_some_and(|c| SEPARATORS.contains(c)) {
        out.push(b'\\'.into());
    }
    let verbatim = !prefix.is_empty();
    out.extend(tail.iter().map(|&c| {
        if verbatim && c == u16::from(b'/') {
            b'\\'.into()
        } else {
            c
        }
    }));
    out.push(0);
    Some(out)
}

/// A title's wide path: the host path when it is on a virtual drive, else
/// the title's own pointer.
struct Wide(*const u16, Option<Vec<u16>>);

impl Wide {
    fn ptr(&self) -> *const u16 {
        self.1.as_ref().map_or(self.0, |p| p.as_ptr())
    }
}

fn wide(path: *const u16) -> Wide {
    if path.is_null() {
        return Wide(path, None);
    }
    // SAFETY: the title passes a NUL-terminated path.
    let path_slice = unsafe {
        let len = (0..).take_while(|&i| *path.add(i) != 0).count();
        std::slice::from_raw_parts(path, len)
    };
    let host = translate(path_slice);
    trace(path_slice, host.as_deref());
    Wide(path, host)
}

/// Every title path at debug level, with the host path if rewritten.
fn trace(path: &[u16], host: Option<&[u16]>) {
    tracing::debug!(
        path = %String::from_utf16_lossy(path),
        host = host.map(|h| String::from_utf16_lossy(&h[..h.len() - 1])),
        "title path"
    );
}

/// A narrow path (ANSI code page, as the host decodes it) on a virtual
/// drive, as a wide host path.
fn narrow(path: *const c_char) -> Option<Vec<u16>> {
    if path.is_null() {
        return None;
    }
    // SAFETY: the title passes a NUL-terminated path.
    let bytes = unsafe { CStr::from_ptr(path) }.to_bytes();
    // The drive part is ASCII in every code page; decode only candidates.
    match bytes.strip_prefix(br"\\?\").unwrap_or(bytes) {
        [letter, b':', ..] if letter.is_ascii_alphabetic() => {}
        _ => {
            tracing::debug!(path = %String::from_utf8_lossy(bytes), "title path");
            return None;
        }
    }
    let len = i32::try_from(bytes.len()).ok()?;
    // SAFETY: sizing call, then a call with room for `n` units.
    let wide = unsafe {
        let n = MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), len, std::ptr::null_mut(), 0);
        let mut wide = vec![0u16; usize::try_from(n).ok()?];
        MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), len, wide.as_mut_ptr(), n);
        wide
    };
    let host = translate(&wide);
    trace(&wide, host.as_deref());
    host
}

/// An ASCII mode string (`"rb"`, `"w, ccs=UTF-8"`) for a wide CRT call.
fn widen(mode: *const c_char) -> Vec<u16> {
    if mode.is_null() {
        return vec![0];
    }
    // SAFETY: a NUL-terminated mode string.
    let bytes = unsafe { CStr::from_ptr(mode) }.to_bytes();
    bytes.iter().map(|&b| u16::from(b)).chain([0]).collect()
}

type Handle = *mut c_void;
type File = *mut c_void;

// kernel32 and its file API sets.

pub extern "system" fn CreateFileW(
    path: *const u16,
    access: u32,
    share: u32,
    security: *const c_void,
    disposition: u32,
    flags: u32,
    template: Handle,
) -> Handle {
    let p = wide(path);
    host!("kernel32.dll", "CreateFileW": extern "system" fn(*const u16, u32, u32, *const c_void, u32, u32, Handle) -> Handle)(
        p.ptr(),
        access,
        share,
        security,
        disposition,
        flags,
        template,
    )
}

pub extern "system" fn CreateFileA(
    path: *const c_char,
    access: u32,
    share: u32,
    security: *const c_void,
    disposition: u32,
    flags: u32,
    template: Handle,
) -> Handle {
    match narrow(path) {
        Some(p) => {
            host!("kernel32.dll", "CreateFileW": extern "system" fn(*const u16, u32, u32, *const c_void, u32, u32, Handle) -> Handle)(
                p.as_ptr(),
                access,
                share,
                security,
                disposition,
                flags,
                template,
            )
        }
        None => {
            host!("kernel32.dll", "CreateFileA": extern "system" fn(*const c_char, u32, u32, *const c_void, u32, u32, Handle) -> Handle)(
                path,
                access,
                share,
                security,
                disposition,
                flags,
                template,
            )
        }
    }
}

/// Used by Bink.
pub extern "system" fn CreateFile2(
    path: *const u16,
    access: u32,
    share: u32,
    disposition: u32,
    params: *const c_void,
) -> Handle {
    let p = wide(path);
    host!("kernel32.dll", "CreateFile2": extern "system" fn(*const u16, u32, u32, u32, *const c_void) -> Handle)(
        p.ptr(),
        access,
        share,
        disposition,
        params,
    )
}

pub extern "system" fn DeleteFileA(path: *const c_char) -> i32 {
    match narrow(path) {
        Some(p) => {
            host!("kernel32.dll", "DeleteFileW": extern "system" fn(*const u16) -> i32)(p.as_ptr())
        }
        None => {
            host!("kernel32.dll", "DeleteFileA": extern "system" fn(*const c_char) -> i32)(path)
        }
    }
}

pub extern "system" fn DeleteFileW(path: *const u16) -> i32 {
    let p = wide(path);
    host!("kernel32.dll", "DeleteFileW": extern "system" fn(*const u16) -> i32)(p.ptr())
}

pub extern "system" fn RemoveDirectoryW(path: *const u16) -> i32 {
    let p = wide(path);
    host!("kernel32.dll", "RemoveDirectoryW": extern "system" fn(*const u16) -> i32)(p.ptr())
}

pub extern "system" fn SetFileAttributesW(path: *const u16, attributes: u32) -> i32 {
    let p = wide(path);
    host!("kernel32.dll", "SetFileAttributesW": extern "system" fn(*const u16, u32) -> i32)(
        p.ptr(),
        attributes,
    )
}

pub extern "system" fn GetFileAttributesA(path: *const c_char) -> u32 {
    match narrow(path) {
        Some(p) => {
            host!("kernel32.dll", "GetFileAttributesW": extern "system" fn(*const u16) -> u32)(
                p.as_ptr(),
            )
        }
        None => {
            host!("kernel32.dll", "GetFileAttributesA": extern "system" fn(*const c_char) -> u32)(
                path,
            )
        }
    }
}

pub extern "system" fn CreateDirectoryW(path: *const u16, security: *const c_void) -> i32 {
    let p = wide(path);
    host!("kernel32.dll", "CreateDirectoryW": extern "system" fn(*const u16, *const c_void) -> i32)(
        p.ptr(),
        security,
    )
}

pub extern "system" fn GetFileAttributesExW(
    path: *const u16,
    level: i32,
    info: *mut c_void,
) -> i32 {
    let p = wide(path);
    host!("kernel32.dll", "GetFileAttributesExW": extern "system" fn(*const u16, i32, *mut c_void) -> i32)(
        p.ptr(),
        level,
        info,
    )
}

pub extern "system" fn GetFileAttributesW(path: *const u16) -> u32 {
    let p = wide(path);
    host!("kernel32.dll", "GetFileAttributesW": extern "system" fn(*const u16) -> u32)(p.ptr())
}

pub extern "system" fn GetDiskFreeSpaceExW(
    path: *const u16,
    available: *mut u64,
    total: *mut u64,
    free: *mut u64,
) -> i32 {
    let p = wide(path);
    host!("kernel32.dll", "GetDiskFreeSpaceExW": extern "system" fn(*const u16, *mut u64, *mut u64, *mut u64) -> i32)(
        p.ptr(),
        available,
        total,
        free,
    )
}

/// Wants a volume root: a virtual drive answers for the host volume its
/// folder is on.
pub extern "system" fn GetDiskFreeSpaceW(
    root: *const u16,
    sectors_per_cluster: *mut u32,
    bytes_per_sector: *mut u32,
    free_clusters: *mut u32,
    clusters: *mut u32,
) -> i32 {
    let mut p = wide(root);
    if let Some(host) = &mut p.1
        && let [letter, colon, sep, ..] = host[..]
        && colon == u16::from(b':')
        && SEPARATORS.contains(&sep)
    {
        *host = vec![letter, colon, sep, 0];
    }
    host!("kernel32.dll", "GetDiskFreeSpaceW": extern "system" fn(*const u16, *mut u32, *mut u32, *mut u32, *mut u32) -> i32)(
        p.ptr(),
        sectors_per_cluster,
        bytes_per_sector,
        free_clusters,
        clusters,
    )
}

pub extern "system" fn FindFirstFileW(path: *const u16, data: *mut c_void) -> Handle {
    let p = wide(path);
    host!("kernel32.dll", "FindFirstFileW": extern "system" fn(*const u16, *mut c_void) -> Handle)(
        p.ptr(),
        data,
    )
}

pub extern "system" fn FindFirstFileExW(
    path: *const u16,
    level: i32,
    data: *mut c_void,
    search: i32,
    filter: *mut c_void,
    flags: u32,
) -> Handle {
    let p = wide(path);
    host!("kernel32.dll", "FindFirstFileExW": extern "system" fn(*const u16, i32, *mut c_void, i32, *mut c_void, u32) -> Handle)(
        p.ptr(),
        level,
        data,
        search,
        filter,
        flags,
    )
}

pub extern "system" fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32 {
    let (from, to) = (wide(from), wide(to));
    host!("kernel32.dll", "MoveFileExW": extern "system" fn(*const u16, *const u16, u32) -> i32)(
        from.ptr(),
        to.ptr(),
        flags,
    )
}

pub extern "system" fn SetCurrentDirectoryA(path: *const c_char) -> i32 {
    match narrow(path) {
        Some(p) => {
            host!("kernel32.dll", "SetCurrentDirectoryW": extern "system" fn(*const u16) -> i32)(
                p.as_ptr(),
            )
        }
        None => {
            host!("kernel32.dll", "SetCurrentDirectoryA": extern "system" fn(*const c_char) -> i32)(
                path,
            )
        }
    }
}

// The C runtime (ucrtbase, which the title's CRT imports bind to).

pub extern "C" fn _wfopen(path: *const u16, mode: *const u16) -> File {
    let p = wide(path);
    host!("ucrtbase.dll", "_wfopen": extern "C" fn(*const u16, *const u16) -> File)(p.ptr(), mode)
}

pub extern "C" fn _wfopen_s(out: *mut File, path: *const u16, mode: *const u16) -> i32 {
    let p = wide(path);
    host!("ucrtbase.dll", "_wfopen_s": extern "C" fn(*mut File, *const u16, *const u16) -> i32)(
        out,
        p.ptr(),
        mode,
    )
}

pub extern "C" fn _wfsopen(path: *const u16, mode: *const u16, share: i32) -> File {
    let p = wide(path);
    host!("ucrtbase.dll", "_wfsopen": extern "C" fn(*const u16, *const u16, i32) -> File)(
        p.ptr(),
        mode,
        share,
    )
}

// Kernel reads into title memory. The GPU side leaves blocks of title GPU
// buffers read-only to see the title's next write, and the kernel's writes
// fail there instead of faulting into its handler; the title's
// synchronous reads (title+0x313fc0) read straight into 4 KiB-aligned
// destinations and retry a failed `ReadFile` forever. So the destination
// is opened first, and not guarded again before the call is done.

pub extern "system" fn ReadFile(
    file: *mut c_void,
    buffer: *mut u8,
    len: u32,
    read: *mut u32,
    overlapped: *mut c_void,
) -> i32 {
    let writing = victory_gpu::kernel_writing(buffer as usize, len as usize);
    let host_read = host!("kernel32.dll", "ReadFile": extern "system" fn(*mut c_void, *mut u8, u32, *mut u32, *mut c_void) -> i32);
    // If an earlier unaligned bounce read on a `FILE_FLAG_NO_BUFFERING` handle
    // left a tracked non-sector-aligned logical position (with the kernel file
    // pointer parked at a sector-aligned sentinel), serve synchronous reads on
    // that handle from the tracked logical offset even when `buffer` and `len`
    // happen to be sector-aligned.
    if overlapped.is_null()
        && !buffer.is_null()
        && len > 0
        && UNALIGNED_COUNT.load(std::sync::atomic::Ordering::Relaxed) > 0
        && has_tracked_unaligned_pos(file)
        && let Some(result) = read_file_sync_fallback(host_read, file, buffer, len, read)
    {
        drop(writing);
        return match result {
            Ok(()) => {
                // SAFETY: plain Win32 call.
                unsafe { windows_sys::Win32::Foundation::SetLastError(0) };
                1
            }
            Err(e) => {
                // SAFETY: plain Win32 call.
                unsafe { windows_sys::Win32::Foundation::SetLastError(e) };
                0
            }
        };
    }

    let mut done = host_read(file, buffer, len, read, overlapped);
    // The call's error is the title's to ask for.
    // SAFETY: plain calls.
    let mut error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
    if done == 0
        && error == windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER
        && overlapped.is_null()
        && !buffer.is_null()
        && len > 0
        && let Some(result) = read_file_sync_fallback(host_read, file, buffer, len, read)
    {
        match result {
            Ok(()) => {
                done = 1;
                error = 0;
            }
            Err(e) => {
                error = e;
            }
        }
    }
    // shortcut: a read that goes on after the call (overlapped) is written
    // into pages the kernel has locked by now: it lands all the same, but a
    // refresh that guards its blocks before it does will not see it.
    if done == 0 && error == windows_sys::Win32::Foundation::ERROR_IO_PENDING {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| tracing::info!(len, "ReadFile: a read of the title's goes on after the call"));
    }
    if done == 0 && error == windows_sys::Win32::Foundation::ERROR_NOACCESS {
        static FAILED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = FAILED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if n.is_power_of_two() {
            tracing::warn!(n, buffer = format_args!("{buffer:p}"), len, "ReadFile: the title's buffer could not be written");
        }
    }
    drop(writing);
    // SAFETY: as above.
    unsafe { windows_sys::Win32::Foundation::SetLastError(error) };
    done
}

/// 4 KiB-aligned scratch allocation for unaligned `FILE_FLAG_NO_BUFFERING`
/// bounce reads.
struct AlignedBuf {
    ptr: *mut u8,
    layout: std::alloc::Layout,
}

impl AlignedBuf {
    fn new(size: usize, align: usize) -> Option<Self> {
        let layout = std::alloc::Layout::from_size_align(size, align).ok()?;
        // SAFETY: `size > 0` and `align` is a power of two.
        let ptr = unsafe { std::alloc::alloc(layout) };
        (!ptr.is_null()).then_some(Self { ptr, layout })
    }
}

impl Drop for AlignedBuf {
    fn drop(&mut self) {
        // SAFETY: allocated with the same layout in `new`.
        unsafe { std::alloc::dealloc(self.ptr, self.layout) };
    }
}

/// Issues a single `ReadFile` at `offset` using a local `OVERLAPPED` and
/// waits for completion. Returns `Ok(0)` on `ERROR_HANDLE_EOF` so callers get
/// synchronous `ReadFile` EOF semantics.
fn read_at_overlapped(
    host_read: extern "system" fn(*mut c_void, *mut u8, u32, *mut u32, *mut c_void) -> i32,
    file: *mut c_void,
    offset: u64,
    buffer: *mut u8,
    len: u32,
    event: *mut c_void,
) -> Result<u32, u32> {
    use windows_sys::Win32::Foundation::{ERROR_HANDLE_EOF, ERROR_IO_PENDING, GetLastError};
    use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
    // SAFETY: zero-initialized Win32 OVERLAPPED with valid manual-reset event.
    let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
    ov.Anonymous.Anonymous.Offset = offset as u32;
    ov.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
    ov.hEvent = event;
    let mut got = 0u32;
    let ok = host_read(file, buffer, len, &mut got, (&raw mut ov).cast());
    if ok != 0 {
        return Ok(got);
    }
    // SAFETY: plain call.
    let err = unsafe { GetLastError() };
    if err == ERROR_HANDLE_EOF {
        return Ok(0);
    }
    if err != ERROR_IO_PENDING {
        return Err(err);
    }
    // SAFETY: `ov` is live until `GetOverlappedResult` completes with bWait = 1.
    let waited = unsafe { GetOverlappedResult(file, &ov, &mut got, 1) };
    if waited != 0 {
        return Ok(got);
    }
    // SAFETY: plain call.
    let wait_err = unsafe { GetLastError() };
    if wait_err == ERROR_HANDLE_EOF {
        Ok(0)
    } else {
        Err(wait_err)
    }
}

/// Reads a file handle's `CurrentByteOffset` via `NtQueryInformationFile`
/// (`FilePositionInformation`). Unlike `SetFilePointerEx(..., 0, FILE_CURRENT)`,
/// this succeeds on `FILE_FLAG_NO_BUFFERING` handles even when a short read at
/// EOF left `CurrentByteOffset` at a non-sector-aligned byte offset.
fn query_file_position(file: *mut c_void) -> Option<u64> {
    #[repr(C)]
    struct IoStatusBlock {
        status: i32,
        _pad: u32,
        information: usize,
    }
    const FILE_POSITION_INFORMATION: u32 = 14;
    let nt_query = host!(
        "ntdll.dll",
        "NtQueryInformationFile":
            extern "system" fn(*mut c_void, *mut IoStatusBlock, *mut c_void, u32, u32) -> i32
    );
    let mut iosb = IoStatusBlock {
        status: 0,
        _pad: 0,
        information: 0,
    };
    let mut pos = 0i64;
    let status = nt_query(
        file,
        &mut iosb,
        (&raw mut pos).cast(),
        std::mem::size_of::<i64>() as u32,
        FILE_POSITION_INFORMATION,
    );
    (status >= 0 && pos >= 0).then_some(pos as u64)
}

/// Non-sector-aligned logical byte offset tracked for a `FILE_FLAG_NO_BUFFERING`
/// handle after an unaligned synchronous bounce read. Because Windows forbids
/// setting `CurrentByteOffset` to a non-sector-aligned value on unbuffered
/// handles, we park the kernel `CurrentByteOffset` at a sector-aligned sentinel
/// past EOF; if the caller later seeks the handle (changing `CurrentByteOffset`
/// away from `sentinel_kernel_pos`) or closes and reuses the handle value for
/// another file (`identity` mismatch), the entry automatically invalidates.
#[derive(Clone, Copy)]
struct UnalignedPos {
    identity: (u32, u32, u32),
    sentinel_kernel_pos: u64,
    logical_pos: u64,
}

static UNALIGNED_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static UNALIGNED_POS: std::sync::Mutex<Option<std::collections::HashMap<usize, UnalignedPos>>> =
    std::sync::Mutex::new(None);

fn file_identity(file: *mut c_void) -> Option<(u32, u32, u32)> {
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    // SAFETY: zero-initialized Win32 struct populated by GetFileInformationByHandle.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: queries metadata on a caller-supplied handle (fails cleanly if invalid).
    (unsafe { GetFileInformationByHandle(file, &mut info) } != 0).then_some((
        info.dwVolumeSerialNumber,
        info.nFileIndexHigh,
        info.nFileIndexLow,
    ))
}

fn has_tracked_unaligned_pos(file: *mut c_void) -> bool {
    let Some(kernel_pos) = query_file_position(file) else {
        return false;
    };
    let id = file_identity(file);
    let mut guard = UNALIGNED_POS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(map) = guard.as_mut() else {
        return false;
    };
    if let Some(&entry) = map.get(&(file as usize)) {
        if entry.sentinel_kernel_pos == kernel_pos && id == Some(entry.identity) {
            return true;
        }
        map.remove(&(file as usize));
        UNALIGNED_COUNT.store(map.len(), std::sync::atomic::Ordering::Relaxed);
    }
    false
}

fn effective_file_position(file: *mut c_void) -> Option<u64> {
    let kernel_pos = query_file_position(file)?;
    if UNALIGNED_COUNT.load(std::sync::atomic::Ordering::Relaxed) == 0 {
        return Some(kernel_pos);
    }
    let id = file_identity(file);
    let mut guard = UNALIGNED_POS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(map) = guard.as_mut()
        && let Some(&entry) = map.get(&(file as usize))
    {
        if entry.sentinel_kernel_pos == kernel_pos && id == Some(entry.identity) {
            return Some(entry.logical_pos);
        }
        map.remove(&(file as usize));
        UNALIGNED_COUNT.store(map.len(), std::sync::atomic::Ordering::Relaxed);
    }
    Some(kernel_pos)
}

fn clear_unaligned_pos(file: *mut c_void) {
    if UNALIGNED_COUNT.load(std::sync::atomic::Ordering::Relaxed) == 0 {
        return;
    }
    let mut guard = UNALIGNED_POS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(map) = guard.as_mut()
        && map.remove(&(file as usize)).is_some()
    {
        UNALIGNED_COUNT.store(map.len(), std::sync::atomic::Ordering::Relaxed);
    }
}

fn store_unaligned_pos(file: *mut c_void, sentinel_kernel_pos: u64, logical_pos: u64) {
    let Some(identity) = file_identity(file) else {
        return;
    };
    let mut guard = UNALIGNED_POS.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(std::collections::HashMap::new);
    if map.len() >= 1024 && !map.contains_key(&(file as usize)) {
        map.clear();
    }
    map.insert(
        file as usize,
        UnalignedPos {
            identity,
            sentinel_kernel_pos,
            logical_pos,
        },
    );
    UNALIGNED_COUNT.store(map.len(), std::sync::atomic::Ordering::Relaxed);
}

/// Advances or sets `file`'s byte offset to `target`. When `SetFilePointerEx`
/// rejects a non-sector-aligned offset on a `FILE_FLAG_NO_BUFFERING` handle,
/// parks the kernel pointer at a sector-aligned sentinel past EOF and records
/// `target` in [`UNALIGNED_POS`].
fn set_file_position(file: *mut c_void, target: u64, file_size_hint: Option<u64>) -> bool {
    use windows_sys::Win32::Storage::FileSystem::{FILE_BEGIN, GetFileSizeEx};
    const SECTOR: u64 = 4096;
    let host_set = host!(
        "kernel32.dll",
        "SetFilePointerEx": extern "system" fn(*mut c_void, i64, *mut i64, u32) -> i32
    );
    if host_set(file, target as i64, std::ptr::null_mut(), FILE_BEGIN) != 0 {
        clear_unaligned_pos(file);
        return true;
    }
    let size_u = match file_size_hint {
        Some(s) => s,
        None => {
            let mut file_size = 0i64;
            // SAFETY: queries size of a caller-supplied handle.
            if unsafe { GetFileSizeEx(file, &mut file_size) } != 0 && file_size >= 0 {
                file_size as u64
            } else {
                target
            }
        }
    };
    let sentinel = size_u.max(target).next_multiple_of(SECTOR) + SECTOR;
    if host_set(file, sentinel as i64, std::ptr::null_mut(), FILE_BEGIN) != 0 {
        store_unaligned_pos(file, sentinel, target);
        return true;
    }
    false
}

/// Hooks `SetFilePointerEx` so `FILE_CURRENT` queries and relative seeks on a
/// `FILE_FLAG_NO_BUFFERING` handle whose kernel pointer was parked at a
/// sector-aligned sentinel report and advance from the tracked logical offset.
pub extern "system" fn SetFilePointerEx(
    file: Handle,
    distance: i64,
    new_pos: *mut i64,
    method: u32,
) -> i32 {
    use windows_sys::Win32::Foundation::{
        ERROR_INVALID_PARAMETER, ERROR_NEGATIVE_SEEK, GetLastError, SetLastError,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_BEGIN, FILE_CURRENT, FILE_END, GetFileSizeEx,
    };
    let host_set = host!(
        "kernel32.dll",
        "SetFilePointerEx": extern "system" fn(Handle, i64, *mut i64, u32) -> i32
    );
    if UNALIGNED_COUNT.load(std::sync::atomic::Ordering::Relaxed) == 0
        || !has_tracked_unaligned_pos(file)
    {
        return host_set(file, distance, new_pos, method);
    }

    if method == FILE_CURRENT {
        let Some(cur) = effective_file_position(file) else {
            return host_set(file, distance, new_pos, method);
        };
        let Some(target) = i64::try_from(cur).ok().and_then(|c| c.checked_add(distance)) else {
            // SAFETY: plain Win32 call.
            unsafe { SetLastError(ERROR_NEGATIVE_SEEK) };
            return 0;
        };
        if target < 0 {
            // SAFETY: plain Win32 call.
            unsafe { SetLastError(ERROR_NEGATIVE_SEEK) };
            return 0;
        }
        if distance != 0 && !set_file_position(file, target as u64, None) {
            return 0;
        }
        if !new_pos.is_null() {
            // SAFETY: checked non-null above.
            unsafe { *new_pos = target };
        }
        // SAFETY: plain Win32 call.
        unsafe { SetLastError(0) };
        return 1;
    }

    let ok = host_set(file, distance, new_pos, method);
    if ok != 0 {
        clear_unaligned_pos(file);
        return ok;
    }
    // SAFETY: plain Win32 call.
    let err = unsafe { GetLastError() };
    if err == ERROR_INVALID_PARAMETER && (method == FILE_BEGIN || method == FILE_END) {
        let mut file_size = 0i64;
        // SAFETY: queries size of a caller-supplied handle.
        let has_size = unsafe { GetFileSizeEx(file, &mut file_size) } != 0 && file_size >= 0;
        let target = if method == FILE_BEGIN {
            Some(distance)
        } else if has_size {
            file_size.checked_add(distance)
        } else {
            None
        };
        if let Some(target) = target {
            if target < 0 {
                // SAFETY: plain Win32 call.
                unsafe { SetLastError(ERROR_NEGATIVE_SEEK) };
                return 0;
            }
            if set_file_position(file, target as u64, has_size.then_some(file_size as u64)) {
                if !new_pos.is_null() {
                    // SAFETY: checked non-null above.
                    unsafe { *new_pos = target };
                }
                // SAFETY: plain Win32 call.
                unsafe { SetLastError(0) };
                return 1;
            }
        }
    }
    // SAFETY: restore original error from host_set.
    unsafe { SetLastError(err) };
    0
}

/// Hooks `SetFilePointer` with the same tracked-offset handling as
/// [`SetFilePointerEx`].
pub extern "system" fn SetFilePointer(
    file: Handle,
    distance_low: i32,
    distance_high: *mut i32,
    method: u32,
) -> u32 {
    use windows_sys::Win32::Foundation::SetLastError;
    const INVALID_SET_FILE_POINTER: u32 = u32::MAX;
    if UNALIGNED_COUNT.load(std::sync::atomic::Ordering::Relaxed) == 0
        || !has_tracked_unaligned_pos(file)
    {
        return host!(
            "kernel32.dll",
            "SetFilePointer": extern "system" fn(Handle, i32, *mut i32, u32) -> u32
        )(file, distance_low, distance_high, method);
    }
    let distance = if distance_high.is_null() {
        i64::from(distance_low)
    } else {
        // SAFETY: checked non-null above.
        let high = unsafe { *distance_high };
        (i64::from(high) << 32) | i64::from(distance_low as u32)
    };
    let mut new_pos = 0i64;
    if SetFilePointerEx(file, distance, &mut new_pos, method) == 0 {
        return INVALID_SET_FILE_POINTER;
    }
    if !distance_high.is_null() {
        // SAFETY: checked non-null above.
        unsafe { *distance_high = (new_pos >> 32) as i32 };
    }
    // SAFETY: required by Win32 SetFilePointer when low 32 bits equal INVALID_SET_FILE_POINTER.
    unsafe { SetLastError(0) };
    new_pos as u32
}

/// Fallback when a synchronous `ReadFile(..., NULL)` fails with
/// `ERROR_INVALID_PARAMETER` (87):
/// 1. Handles opened with `FILE_FLAG_OVERLAPPED` reject NULL `OVERLAPPED` on
///    Windows (the console accepts synchronous reads on them): retry via a
///    local `OVERLAPPED` at the handle's current byte offset and advance it.
/// 2. Handles opened with `FILE_FLAG_NO_BUFFERING` (with or without
///    `FILE_FLAG_OVERLAPPED`) reject unaligned top-up/tail reads (e.g. a
///    short-read retry at EOF or unaligned buffer/length): answer EOF with 0
///    bytes or bounce through a 4 KiB-aligned buffer.
///
/// Returns `None` when the handle is not a seekable file so genuinely invalid
/// calls keep their original `ERROR_INVALID_PARAMETER`.
fn read_file_sync_fallback(
    host_read: extern "system" fn(*mut c_void, *mut u8, u32, *mut u32, *mut c_void) -> i32,
    file: *mut c_void,
    buffer: *mut u8,
    len: u32,
    read: *mut u32,
) -> Option<Result<(), u32>> {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER};
    use windows_sys::Win32::Storage::FileSystem::GetFileSizeEx;
    use windows_sys::Win32::System::Threading::CreateEventW;

    let pos_u = effective_file_position(file)?;
    // SAFETY: creates a private manual-reset event closed before return.
    let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if event.is_null() {
        return None;
    }
    struct EventGuard(*mut c_void);
    impl Drop for EventGuard {
        fn drop(&mut self) {
            // SAFETY: valid handle owned by this guard.
            unsafe { CloseHandle(self.0) };
        }
    }
    let _guard = EventGuard(event);

    match read_at_overlapped(host_read, file, pos_u, buffer, len, event) {
        Ok(got) => {
            let next_pos = pos_u + u64::from(got);
            set_file_position(file, next_pos, None);
            if !read.is_null() {
                // SAFETY: checked non-null above.
                unsafe { *read = got };
            }
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| {
                tracing::info!(
                    len,
                    got,
                    "ReadFile: synchronous read on overlapped handle completed via local OVERLAPPED"
                );
            });
            return Some(Ok(()));
        }
        Err(e) if e != ERROR_INVALID_PARAMETER => return Some(Err(e)),
        Err(_) => {}
    }

    // Still ERROR_INVALID_PARAMETER: likely `FILE_FLAG_NO_BUFFERING` with
    // unaligned offset, length, or destination buffer.
    let mut file_size = 0i64;
    // SAFETY: queries size of a valid file handle.
    if unsafe { GetFileSizeEx(file, &mut file_size) } == 0 || file_size < 0 {
        return None;
    }
    let file_size_u = file_size as u64;
    if pos_u >= file_size_u {
        if !read.is_null() {
            // SAFETY: checked non-null above.
            unsafe { *read = 0 };
        }
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            tracing::info!(
                pos = pos_u,
                file_size = file_size_u,
                len,
                "ReadFile: unaligned read at EOF on unbuffered handle answered with 0 bytes"
            );
        });
        return Some(Ok(()));
    }

    const SECTOR: u64 = 4096;
    const CHUNK_MAX: u64 = 1 << 20;
    let remaining = (u64::from(len)).min(file_size_u - pos_u);
    let mut copied = 0u64;
    while copied < remaining {
        let cur = pos_u + copied;
        let aligned_start = cur & !(SECTOR - 1);
        let lead = (cur - aligned_start) as usize;
        let want = remaining - copied;
        let raw_need = (lead as u64 + want).next_multiple_of(SECTOR);
        let chunk_len = raw_need.min(CHUNK_MAX) as usize;
        let bounce = AlignedBuf::new(chunk_len, SECTOR as usize)?;
        let got = match read_at_overlapped(host_read, file, aligned_start, bounce.ptr, chunk_len as u32, event) {
            Ok(g) => g as usize,
            Err(e) => return Some(Err(e)),
        };
        if got <= lead {
            break;
        }
        let take = (got - lead).min(want as usize);
        // SAFETY: `bounce` has `got` valid bytes; `buffer` has `len` writable bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(bounce.ptr.add(lead), buffer.add(copied as usize), take);
        }
        copied += take as u64;
        if got < chunk_len {
            break;
        }
    }
    let copied_u32 = copied as u32;
    let next_pos = pos_u + copied;
    set_file_position(file, next_pos, Some(file_size_u));
    if !read.is_null() {
        // SAFETY: checked non-null above.
        unsafe { *read = copied_u32 };
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        tracing::info!(
            pos = pos_u,
            len,
            got = copied_u32,
            "ReadFile: unaligned read on unbuffered handle completed via 4 KiB-aligned bounce buffer"
        );
    });
    Some(Ok(()))
}

/// The host CRT reads large requests straight into the caller's buffer
/// through the kernel, whose write fails on a guarded block instead of
/// faulting into the guard. So the bytes are read beside, a chunk at a time
/// (a caller may ask for far more than the file holds), and copied in from
/// user mode, where a guarded block faults into the guard like the title's
/// own writes. Returns the items read, as the CRT does; the bytes of a
/// partial last item are copied too, as the CRT leaves them.
pub extern "C" fn fread(buffer: *mut u8, size: usize, count: usize, stream: File) -> usize {
    let _writing = victory_gpu::kernel_writing(buffer as usize, size.saturating_mul(count));
    host!("ucrtbase.dll", "fread": extern "C" fn(*mut u8, usize, usize, File) -> usize)(
        buffer, size, count, stream,
    )
}

pub extern "C" fn fopen(path: *const c_char, mode: *const c_char) -> File {
    match narrow(path) {
        Some(p) => host!("ucrtbase.dll", "_wfopen": extern "C" fn(*const u16, *const u16) -> File)(
            p.as_ptr(),
            widen(mode).as_ptr(),
        ),
        None => {
            host!("ucrtbase.dll", "fopen": extern "C" fn(*const c_char, *const c_char) -> File)(
                path, mode,
            )
        }
    }
}

pub extern "C" fn fopen_s(out: *mut File, path: *const c_char, mode: *const c_char) -> i32 {
    match narrow(path) {
        Some(p) => {
            host!("ucrtbase.dll", "_wfopen_s": extern "C" fn(*mut File, *const u16, *const u16) -> i32)(
                out,
                p.as_ptr(),
                widen(mode).as_ptr(),
            )
        }
        None => {
            host!("ucrtbase.dll", "fopen_s": extern "C" fn(*mut File, *const c_char, *const c_char) -> i32)(
                out, path, mode,
            )
        }
    }
}

/// `_open(path, flags[, mode])` is variadic; on x64 the optional integer
/// travels in the same register either way, so it is passed on as is.
pub extern "C" fn _open(path: *const c_char, flags: i32, mode: i32) -> i32 {
    match narrow(path) {
        Some(p) => host!("ucrtbase.dll", "_wopen": extern "C" fn(*const u16, i32, i32) -> i32)(
            p.as_ptr(),
            flags,
            mode,
        ),
        None => host!("ucrtbase.dll", "_open": extern "C" fn(*const c_char, i32, i32) -> i32)(
            path, flags, mode,
        ),
    }
}

/// See [`_open`]; used by libHttpClient.
pub extern "C" fn _wopen(path: *const u16, flags: i32, mode: i32) -> i32 {
    let p = wide(path);
    host!("ucrtbase.dll", "_wopen": extern "C" fn(*const u16, i32, i32) -> i32)(
        p.ptr(),
        flags,
        mode,
    )
}

pub extern "C" fn _wstat64(path: *const u16, stat: *mut c_void) -> i32 {
    let p = wide(path);
    host!("ucrtbase.dll", "_wstat64": extern "C" fn(*const u16, *mut c_void) -> i32)(p.ptr(), stat)
}

pub extern "C" fn _stat64i32(path: *const c_char, stat: *mut c_void) -> i32 {
    match narrow(path) {
        Some(p) => {
            host!("ucrtbase.dll", "_wstat64i32": extern "C" fn(*const u16, *mut c_void) -> i32)(
                p.as_ptr(),
                stat,
            )
        }
        None => {
            host!("ucrtbase.dll", "_stat64i32": extern "C" fn(*const c_char, *mut c_void) -> i32)(
                path, stat,
            )
        }
    }
}

pub extern "C" fn remove(path: *const c_char) -> i32 {
    match narrow(path) {
        Some(p) => host!("ucrtbase.dll", "_wremove": extern "C" fn(*const u16) -> i32)(p.as_ptr()),
        None => host!("ucrtbase.dll", "remove": extern "C" fn(*const c_char) -> i32)(path),
    }
}

pub extern "C" fn _wchmod(path: *const u16, mode: i32) -> i32 {
    let p = wide(path);
    host!("ucrtbase.dll", "_wchmod": extern "C" fn(*const u16, i32) -> i32)(p.ptr(), mode)
}

pub extern "C" fn _wutime64(path: *const u16, times: *mut c_void) -> i32 {
    let p = wide(path);
    host!("ucrtbase.dll", "_wutime64": extern "C" fn(*const u16, *mut c_void) -> i32)(
        p.ptr(),
        times,
    )
}

// The C++ library: `std::basic_filebuf::open` goes through `_Fiopen`.

const FIOPEN: &str = "?_Fiopen@std@@YAPEAU_iobuf@@PEBDHH@Z";
const FIOPEN_WIDE: &str = "?_Fiopen@std@@YAPEAU_iobuf@@PEB_WHH@Z";

/// `_Fiopen(const wchar_t* path, ios_base::openmode, int protection)`.
extern "C" fn fiopen_wide(path: *const u16, mode: i32, protection: i32) -> File {
    let p = wide(path);
    host!("msvcp140.dll", "?_Fiopen@std@@YAPEAU_iobuf@@PEB_WHH@Z": extern "C" fn(*const u16, i32, i32) -> File)(
        p.ptr(),
        mode,
        protection,
    )
}

/// `_Fiopen(const char* path, ios_base::openmode, int protection)`.
extern "C" fn fiopen(path: *const c_char, mode: i32, protection: i32) -> File {
    match narrow(path) {
        Some(p) => {
            host!("msvcp140.dll", "?_Fiopen@std@@YAPEAU_iobuf@@PEB_WHH@Z": extern "C" fn(*const u16, i32, i32) -> File)(
                p.as_ptr(),
                mode,
                protection,
            )
        }
        None => {
            host!("msvcp140.dll", "?_Fiopen@std@@YAPEAU_iobuf@@PEBDHH@Z": extern "C" fn(*const c_char, i32, i32) -> File)(
                path, mode, protection,
            )
        }
    }
}

pub(crate) fn module() -> HleModule {
    HleModule {
        name: "files",
        dlls: &[
            "kernel32.dll",
            "kernelbase.dll",
            "api-ms-win-core-file-*",
            "api-ms-win-core-processenvironment-*",
            "ucrtbase.dll",
            "api-ms-win-crt-stdio-*",
            "api-ms-win-crt-filesystem-*",
            "api-ms-win-crt-time-*",
            "msvcp140.dll",
        ],
        exports: vec![
            hle_export!(ReadFile),
            hle_export!(SetFilePointer),
            hle_export!(SetFilePointerEx),
            hle_export!(fread),
            hle_export!(CreateFileW),
            hle_export!(CreateFileA),
            hle_export!(CreateFile2),
            hle_export!(DeleteFileA),
            hle_export!(DeleteFileW),
            hle_export!(RemoveDirectoryW),
            hle_export!(SetFileAttributesW),
            hle_export!(GetFileAttributesA),
            hle_export!(CreateDirectoryW),
            hle_export!(GetFileAttributesW),
            hle_export!(GetFileAttributesExW),
            hle_export!(GetDiskFreeSpaceExW),
            hle_export!(GetDiskFreeSpaceW),
            hle_export!(FindFirstFileW),
            hle_export!(FindFirstFileExW),
            hle_export!(MoveFileExW),
            hle_export!(SetCurrentDirectoryA),
            hle_export!(_wfopen),
            hle_export!(_wfopen_s),
            hle_export!(_wfsopen),
            hle_export!(fopen),
            hle_export!(fopen_s),
            hle_export!(_open),
            hle_export!(_wopen),
            hle_export!(_wstat64),
            hle_export!(_stat64i32),
            hle_export!(remove),
            hle_export!(_wchmod),
            hle_export!(_wutime64),
            HleExport {
                name: FIOPEN,
                address: fiopen as *const () as usize,
            },
            HleExport {
                name: FIOPEN_WIDE,
                address: fiopen_wide as *const () as usize,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    #[test]
    fn kernel_reads_reach_the_buffer() {
        use std::os::windows::io::AsRawHandle;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.bin");
        std::fs::write(&path, b"0123456789").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let (mut out, mut n) = ([0u8; 10], 0);
        let handle = file.as_raw_handle().cast();
        assert_ne!(ReadFile(handle, out.as_mut_ptr(), 10, &mut n, std::ptr::null_mut()), 0);
        assert_eq!((&out, n), (b"0123456789", 10));
    }

    #[test]
    fn kernel_reads_overlapped_handle_synchronously() {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("overlapped.bin");
        std::fs::write(&path, b"hello overlapped world").unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OVERLAPPED)
            .open(&path)
            .unwrap();
        let handle = file.as_raw_handle().cast();

        let (mut a, mut n) = ([0u8; 6], 0);
        assert_ne!(ReadFile(handle, a.as_mut_ptr(), 6, &mut n, std::ptr::null_mut()), 0);
        assert_eq!((&a, n), (b"hello ", 6));

        // Sequential second read advances from offset 6.
        let (mut b, mut n2) = ([0u8; 32], 0);
        assert_ne!(ReadFile(handle, b.as_mut_ptr(), 32, &mut n2, std::ptr::null_mut()), 0);
        assert_eq!((&b[..n2 as usize], n2), (&b"overlapped world"[..], 16));

        // Synchronous read at EOF returns TRUE with 0 bytes.
        let (mut c, mut n3) = ([0u8; 8], 99);
        assert_ne!(ReadFile(handle, c.as_mut_ptr(), 8, &mut n3, std::ptr::null_mut()), 0);
        assert_eq!(n3, 0);

        // Synchronous read with NULL `lpNumberOfBytesRead` also succeeds.
        use windows_sys::Win32::Storage::FileSystem::{FILE_BEGIN, SetFilePointerEx};
        assert_ne!(unsafe { SetFilePointerEx(handle, 0, std::ptr::null_mut(), FILE_BEGIN) }, 0);
        let mut d = [0u8; 5];
        assert_ne!(
            ReadFile(handle, d.as_mut_ptr(), 5, std::ptr::null_mut(), std::ptr::null_mut()),
            0
        );
        assert_eq!(&d, b"hello");
    }

    #[test]
    fn kernel_reads_unbuffered_short_read_top_up_and_unaligned_slice() {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_BEGIN, FILE_FLAG_NO_BUFFERING, FILE_FLAG_OVERLAPPED, SetFilePointerEx,
        };

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unbuf.bin");
        // Exact #488 size: 4784 bytes (first 8192 read gets 4784, top-up of 3408 used to fail with 87).
        let payload: Vec<u8> = (0..4784usize).map(|i| ((i * 31 + 7) & 0xff) as u8).collect();
        std::fs::write(&path, &payload).unwrap();

        let big_path = dir.path().join("unbuf_big.bin");
        let big_payload: Vec<u8> = (0..10_000usize).map(|i| ((i * 17 + 3) & 0xff) as u8).collect();
        std::fs::write(&big_path, &big_payload).unwrap();

        for flags in [FILE_FLAG_NO_BUFFERING, FILE_FLAG_NO_BUFFERING | FILE_FLAG_OVERLAPPED] {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(flags)
                .open(&path)
                .unwrap();
            let handle = file.as_raw_handle().cast();

            let mut buf = vec![0u8; 8192];
            let mut got1 = 0u32;
            assert_ne!(ReadFile(handle, buf.as_mut_ptr(), 8192, &mut got1, std::ptr::null_mut()), 0);
            assert_eq!(got1 as usize, payload.len());
            assert_eq!(&buf[..payload.len()], &payload[..]);

            // Unaligned top-up read at EOF (3408 bytes into buf + 4784) succeeds with 0 bytes.
            let mut got2 = 99u32;
            let ok2 = ReadFile(
                handle,
                // SAFETY: within `buf` (4784 + 3408 == 8192).
                unsafe { buf.as_mut_ptr().add(4784) },
                3408,
                &mut got2,
                std::ptr::null_mut(),
            );
            assert_ne!(
                ok2,
                0,
                "flags={flags:#x} err={}",
                unsafe { windows_sys::Win32::Foundation::GetLastError() }
            );
            assert_eq!(got2, 0);

            // Sequential unaligned reads (0..3408, then 3408..4784, then EOF).
            let file2 = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(flags)
                .open(&path)
                .unwrap();
            let h2 = file2.as_raw_handle().cast();
            let mut slice = vec![0u8; 3408];
            let mut n_slice = 0u32;
            assert_ne!(ReadFile(h2, slice.as_mut_ptr(), 3408, &mut n_slice, std::ptr::null_mut()), 0);
            assert_eq!(n_slice, 3408);
            assert_eq!(&slice[..], &payload[..3408]);

            let mut tail = vec![0u8; 2000];
            let mut n_tail = 0u32;
            assert_ne!(ReadFile(h2, tail.as_mut_ptr(), 2000, &mut n_tail, std::ptr::null_mut()), 0);
            assert_eq!(n_tail, 1376);
            assert_eq!(&tail[..1376], &payload[3408..]);

            let mut eof_n = 99u32;
            assert_ne!(ReadFile(h2, tail.as_mut_ptr(), 64, &mut eof_n, std::ptr::null_mut()), 0);
            assert_eq!(eof_n, 0);

            // Unaligned read followed by a 4 KiB-aligned read AND explicit SetFilePointerEx rewind.
            let file3 = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(flags)
                .open(&big_path)
                .unwrap();
            let h3 = file3.as_raw_handle().cast();
            let mut head = vec![0u8; 1500];
            let mut n_head = 0u32;
            assert_ne!(ReadFile(h3, head.as_mut_ptr(), 1500, &mut n_head, std::ptr::null_mut()), 0);
            assert_eq!(n_head, 1500);
            assert_eq!(&head[..], &big_payload[..1500]);

            let aligned = AlignedBuf::new(4096, 4096).unwrap();
            let mut n_aligned = 0u32;
            assert_ne!(ReadFile(h3, aligned.ptr, 4096, &mut n_aligned, std::ptr::null_mut()), 0);
            assert_eq!(n_aligned, 4096);
            let aligned_slice = unsafe { std::slice::from_raw_parts(aligned.ptr, 4096) };
            assert_eq!(aligned_slice, &big_payload[1500..5596]);

            // Rewinding to 0 invalidates the tracked unaligned position cleanly.
            assert_ne!(unsafe { SetFilePointerEx(h3, 0, std::ptr::null_mut(), FILE_BEGIN) }, 0);
            let mut rewound = vec![0u8; 1500];
            let mut n_rewound = 0u32;
            assert_ne!(
                ReadFile(h3, rewound.as_mut_ptr(), 1500, &mut n_rewound, std::ptr::null_mut()),
                0
            );
            assert_eq!(n_rewound, 1500);
            assert_eq!(&rewound[..], &big_payload[..1500]);
        }
    }

    #[test]
    fn unbuffered_bounce_read_reports_logical_position_via_set_file_pointer() {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_CURRENT, FILE_FLAG_NO_BUFFERING, FILE_FLAG_OVERLAPPED,
        };

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pos.bin");
        let payload: Vec<u8> = (0..10_000usize).map(|i| ((i * 29 + 11) & 0xff) as u8).collect();
        std::fs::write(&path, &payload).unwrap();

        for flags in [FILE_FLAG_NO_BUFFERING, FILE_FLAG_NO_BUFFERING | FILE_FLAG_OVERLAPPED] {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(flags)
                .open(&path)
                .unwrap();
            let handle = file.as_raw_handle().cast();

            let mut buf = vec![0u8; 4784];
            let mut got = 0u32;
            assert_ne!(ReadFile(handle, buf.as_mut_ptr(), 4784, &mut got, std::ptr::null_mut()), 0);
            assert_eq!(got, 4784);
            assert_eq!(&buf[..], &payload[..4784]);

            // Query logical position via SetFilePointerEx and SetFilePointer.
            let mut pos = -1i64;
            assert_ne!(SetFilePointerEx(handle, 0, &mut pos, FILE_CURRENT), 0);
            assert_eq!(pos, 4784);
            assert_eq!(SetFilePointer(handle, 0, std::ptr::null_mut(), FILE_CURRENT), 4784);

            // Relative seek backward to an unaligned offset re-parks and reads from 4000.
            let mut pos2 = -1i64;
            assert_ne!(SetFilePointerEx(handle, -784, &mut pos2, FILE_CURRENT), 0);
            assert_eq!(pos2, 4000);
            let mut slice = [0u8; 128];
            let mut got2 = 0u32;
            assert_ne!(
                ReadFile(handle, slice.as_mut_ptr(), 128, &mut got2, std::ptr::null_mut()),
                0
            );
            assert_eq!(got2, 128);
            assert_eq!(&slice[..], &payload[4000..4128]);

            // Relative seek to a sector-aligned offset (4128 - 32 = 4096) clears the sentinel.
            let mut pos3 = -1i64;
            assert_ne!(SetFilePointerEx(handle, -32, &mut pos3, FILE_CURRENT), 0);
            assert_eq!(pos3, 4096);
            assert!(!has_tracked_unaligned_pos(handle));
        }
    }

    #[test]
    fn fread_reads_beside_in_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.bin");
        // Over two chunks, and not a whole number of 3-byte items.
        let bytes: Vec<u8> = (0..(5 << 19) + 1).map(|i| (i * 7 + i / 251) as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        let fclose = host!("ucrtbase.dll", "fclose": extern "C" fn(File) -> i32);
        let open = || {
            let f = _wfopen(w(&path.to_string_lossy()).as_ptr(), w("rb").as_ptr());
            assert!(!f.is_null());
            f
        };

        // Asked for far more than the file holds: the whole file, in items.
        let f = open();
        let mut out = vec![0xaau8; bytes.len() + 64];
        let items = fread(out.as_mut_ptr(), 3, out.len() / 3, f);
        assert_eq!(items, bytes.len() / 3);
        assert_eq!(&out[..bytes.len()], &bytes[..], "the partial last item's bytes too");
        assert!(out[bytes.len()..].iter().all(|&b| b == 0xaa), "nothing past what was read");
        fclose(f);

        // Exact reads continue where the last one stopped.
        let f = open();
        let mut a = vec![0u8; 1000];
        let mut b = vec![0u8; (1 << 20) + 5];
        assert_eq!(fread(a.as_mut_ptr(), 1000, 1, f), 1);
        assert_eq!(fread(b.as_mut_ptr(), 1, b.len(), f), b.len());
        assert_eq!(a, bytes[..1000]);
        assert_eq!(b, bytes[1000..1000 + b.len()]);
        fclose(f);
    }

    fn text(v: &[u16]) -> String {
        String::from_utf16_lossy(&v[..v.len() - 1])
    }

    #[test]
    fn rewrites_only_paths_on_mounted_drives() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        let letter = mount(dir.path()).unwrap();
        assert_eq!(
            mount(dir.path()),
            Some(letter),
            "mounting twice reuses the letter"
        );
        let tr = |s: String| translate(&w(&s)[..s.encode_utf16().count()]).map(|v| text(&v));
        assert_eq!(
            tr(format!("{letter}:/a/b.txt")),
            Some(format!("{root}/a/b.txt"))
        );
        let lower = letter.to_ascii_lowercase();
        assert_eq!(tr(format!("{lower}:\\a")), Some(format!("{root}\\a")));
        assert_eq!(tr(format!("{letter}:")), Some(format!("{root}\\")));
        assert_eq!(
            tr(format!(r"\\?\{letter}:/a/b")),
            Some(format!(r"\\?\{root}\a\b"))
        );
        assert_eq!(tr(r"C:\Windows".into()), None);
        assert_eq!(tr("relative/path".into()), None);
        assert_eq!(tr(String::new()), None);

        // Fixed letters, as the console's package drive; a later mount of
        // the same letter replaces the earlier one.
        let package = tempfile::tempdir().unwrap();
        mount_at('g', dir.path());
        mount_at('G', package.path());
        assert_eq!(
            tr(r"G:\appdata0.rpf".into()),
            Some(format!(r"{}\appdata0.rpf", package.path().display()))
        );
        assert_ne!(mount(dir.path()), Some('G'), "free letters skip fixed ones");

        // Long ones take the `\\?\` form, cleaned up as the host would.
        let long = "d".repeat(250);
        assert_eq!(
            tr(format!(r"G:/a/./b/../{long}//f")),
            Some(format!(r"\\?\{}\a\{long}\f", package.path().display()))
        );
        assert_eq!(
            verbatim(&w(r"\\server\share\")[..15], &w("/x/../../y")[..10]),
            w(r"\\?\UNC\server\share\y")[..22].to_vec()
        );
    }

    /// A title run by its file name alone has an empty folder: the drive
    /// over it is the current directory, not the current drive's root.
    #[test]
    fn an_empty_root_is_the_current_directory() {
        let letter = mount(Path::new("")).unwrap();
        let path = format!(r"{letter}:\a.rpf");
        let host = translate(&w(&path)[..path.len()]).map(|v| text(&v));
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(host, Some(cwd.join("a.rpf").display().to_string()));
    }

    #[test]
    fn long_title_paths_work_on_the_host() {
        let dir = tempfile::tempdir().unwrap();
        let letter = mount(dir.path()).unwrap();
        let deep = format!("{letter}:/{}/{}", "a".repeat(200), "b".repeat(100));
        let null = std::ptr::null_mut();
        let first = w(&deep[..203]);
        assert_ne!(CreateDirectoryW(first.as_ptr(), null), 0);
        assert_ne!(CreateDirectoryW(w(&deep).as_ptr(), null), 0);
        let file = format!("{deep}/../{}/f.txt", "b".repeat(100));
        let f = _wfopen(w(&file).as_ptr(), w("wb").as_ptr());
        assert!(!f.is_null(), "{file}");
        host!("ucrtbase.dll", "fclose": extern "C" fn(File) -> i32)(f);
        assert_ne!(DeleteFileW(w(&format!("{deep}/f.txt")).as_ptr()), 0);
        assert_ne!(RemoveDirectoryW(w(&deep).as_ptr()), 0);
    }

    #[test]
    fn title_file_calls_land_in_the_host_folder() {
        let dir = tempfile::tempdir().unwrap();
        let letter = mount(dir.path()).unwrap();
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const CREATE_ALWAYS: u32 = 2;
        let close = host!("kernel32.dll", "CloseHandle": extern "system" fn(Handle) -> i32);
        let fclose = host!("ucrtbase.dll", "fclose": extern "C" fn(File) -> i32);

        let path = w(&format!(r"{letter}:\created.bin"));
        let null = std::ptr::null_mut();
        let h = CreateFileW(
            path.as_ptr(),
            GENERIC_WRITE,
            0,
            null,
            CREATE_ALWAYS,
            0,
            null,
        );
        assert_ne!(h as isize, -1);
        close(h);
        assert!(dir.path().join("created.bin").is_file());

        let path = format!("{letter}:/opened.txt\0");
        let f = fopen(path.as_ptr().cast(), c"wb".as_ptr());
        assert!(!f.is_null());
        fclose(f);
        assert!(dir.path().join("opened.txt").is_file());

        let path = format!("{letter}:/stream.txt\0");
        const OUT: i32 = 2;
        let f = fiopen(path.as_ptr().cast(), OUT, 0x40);
        assert!(!f.is_null());
        fclose(f);
        assert!(dir.path().join("stream.txt").is_file());

        assert_eq!(remove(path.as_ptr().cast()), 0);
        assert!(!dir.path().join("stream.txt").exists());
        let f = fiopen_wide(w(&format!("{letter}:/wide.txt")).as_ptr(), OUT, 0x40);
        assert!(!f.is_null());
        fclose(f);
        assert!(dir.path().join("wide.txt").is_file());

        const OPEN_EXISTING: u32 = 3;
        let path = w(&format!(r"{letter}:\created.bin"));
        let h = CreateFile2(path.as_ptr(), GENERIC_WRITE, 0, OPEN_EXISTING, null);
        assert_ne!(h as isize, -1);
        close(h);
        const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
        assert_ne!(GetFileAttributesW(path.as_ptr()), INVALID_FILE_ATTRIBUTES);

        const O_WRONLY: i32 = 1;
        const O_CREAT: i32 = 0x100;
        const S_IWRITE: i32 = 0x80;
        let path = format!("{letter}:/low.bin\0");
        let fd = _open(path.as_ptr().cast(), O_WRONLY | O_CREAT, S_IWRITE);
        assert!(fd >= 0);
        host!("ucrtbase.dll", "_close": extern "C" fn(i32) -> i32)(fd);
        assert!(dir.path().join("low.bin").is_file());

        // Other paths pass through untouched.
        let host_path = format!("{}\0", dir.path().join("plain.txt").display());
        let f = fopen(host_path.as_ptr().cast(), c"wb".as_ptr());
        assert!(!f.is_null());
        fclose(f);
        assert!(dir.path().join("plain.txt").is_file());
    }

    /// A narrow path is rewritten once, as its wide twin is. The inner
    /// drive's folder lies on the outer virtual drive, as the title folder
    /// does when it sits on a host `G:` drive and is mounted as `G:`. The
    /// outer letter is no host drive, so the host finds nothing there; a
    /// narrow call that reaches the outer folder rewrote the path twice.
    #[test]
    fn narrow_paths_are_rewritten_once() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("inner");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("present.txt"), b"").unwrap();
        let outer = mount(dir.path()).unwrap();
        let inner = mount(Path::new(&format!(r"{outer}:\inner"))).unwrap();
        let path = |name: &str| format!("{inner}:/{name}\0");

        const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
        let wide = GetFileAttributesW(w(&format!("{inner}:/present.txt")).as_ptr());
        assert_eq!(wide, INVALID_FILE_ATTRIBUTES);
        let narrow = GetFileAttributesA(path("present.txt").as_ptr().cast());
        assert_eq!(narrow, wide, "GetFileAttributesA agrees with its wide twin");

        const GENERIC_WRITE: u32 = 0x4000_0000;
        const CREATE_ALWAYS: u32 = 2;
        let null = std::ptr::null_mut();
        let h = CreateFileA(
            path("a.bin").as_ptr().cast(),
            GENERIC_WRITE,
            0,
            null,
            CREATE_ALWAYS,
            0,
            null,
        );
        assert_eq!(h as isize, -1);
        assert!(fopen(path("b.txt").as_ptr().cast(), c"wb".as_ptr()).is_null());
        let mut f: File = std::ptr::null_mut();
        let error = fopen_s(&mut f, path("c.txt").as_ptr().cast(), c"wb".as_ptr());
        assert_ne!(error, 0);
        const OUT: i32 = 2;
        assert!(fiopen(path("d.txt").as_ptr().cast(), OUT, 0x40).is_null());

        // Nothing was made behind a path rewritten twice.
        let names: Vec<_> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["present.txt"]);
    }
}
