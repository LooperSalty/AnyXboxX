//! The few Windows calls the port needs, declared here so the crate has no dependency at all.

use core::ffi::c_void;

pub type Handle = *mut c_void;
pub type Bool = i32;

pub const TH32CS_SNAPTHREAD: u32 = 0x0000_0004;
pub const THREAD_QUERY_INFORMATION: u32 = 0x0000_0040;
/// `ThreadBasicInformation` for NtQueryInformationThread.
pub const THREAD_BASIC_INFORMATION_CLASS: u32 = 0;
pub const INVALID_HANDLE_VALUE: Handle = usize::MAX as Handle;

/// `THREADENTRY32`, field for field.
#[repr(C)]
pub struct ThreadEntry32 {
    pub size: u32,
    pub usage: u32,
    pub thread_id: u32,
    pub owner_process_id: u32,
    pub base_priority: i32,
    pub delta_priority: i32,
    pub flags: u32,
}

#[repr(C)]
pub struct ClientId {
    pub process: Handle,
    pub thread: Handle,
}

/// `THREAD_BASIC_INFORMATION` (x64 layout, 48 bytes); only `teb` is read.
#[repr(C)]
pub struct ThreadBasicInformation {
    pub exit_status: i32,
    pub teb: *mut c_void,
    pub client_id: ClientId,
    pub affinity: usize,
    pub priority: i32,
    pub base_priority: i32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> Handle;
    pub fn Thread32First(snapshot: Handle, entry: *mut ThreadEntry32) -> Bool;
    pub fn Thread32Next(snapshot: Handle, entry: *mut ThreadEntry32) -> Bool;
    pub fn OpenThread(access: u32, inherit_handle: Bool, thread_id: u32) -> Handle;
    pub fn CloseHandle(handle: Handle) -> Bool;
    pub fn GetCurrentProcessId() -> u32;
    pub fn GetCurrentThreadId() -> u32;
    pub fn GetProcessHeap() -> Handle;
    pub fn HeapAlloc(heap: Handle, flags: u32, bytes: usize) -> *mut c_void;
}

#[link(name = "ntdll")]
unsafe extern "system" {
    pub fn NtQueryInformationThread(
        thread: Handle,
        class: u32,
        information: *mut c_void,
        length: u32,
        return_length: *mut u32,
    ) -> i32;
}
