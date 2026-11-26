//! Installation of the game's TLS image into the carrier, and the callbacks that keep new threads
//! in step. Port of `runtime/tls/anyxboxx_tls.cpp` (InstallImageTls, RemoveImageTls, OnThread).

use crate::layout::{self, Plan};
use crate::tls_image::{TLS_INDEX, TlsCallback, TlsDirectory};
use crate::win;
use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_THREAD_ATTACH: u32 = 2;
const DLL_THREAD_DETACH: u32 = 3;
/// `TEB.ThreadLocalStoragePointer` (x64).
const TEB_TLS_POINTER: usize = 0x58;

/// Description of the game's TLS image, as installed. Heap-allocated once and never freed: a
/// thread that started reading it before a remove keeps a valid pointer, as in the C++ DLL.
pub struct GameTls {
    start: *const u8,
    plan: Plan,
    callbacks: *const Option<TlsCallback>,
    base: *mut c_void,
}

static GAME: AtomicPtr<GameTls> = AtomicPtr::new(ptr::null_mut());

/// Address of the calling thread's TEB.
///
/// # Safety
/// Must run on a Windows x64 thread, where the GS segment points to its TEB.
unsafe fn current_teb() -> *mut c_void {
    let teb: *mut c_void;
    // SAFETY: reads the TEB self pointer of the running thread; no memory is written.
    unsafe {
        core::arch::asm!("mov {}, gs:[0x30]", out(reg) teb, options(nostack, readonly, preserves_flags));
    }
    teb
}

/// Zone TLS of this DLL for the thread whose TEB is `teb` (null when the thread has none yet).
///
/// # Safety
/// `teb` must be the TEB of a live thread of this process.
unsafe fn area_of(teb: *mut c_void) -> *mut u8 {
    // SAFETY: TEB.ThreadLocalStoragePointer is an array of block pointers, one per module index.
    unsafe {
        let array = *teb.byte_add(TEB_TLS_POINTER).cast::<*const *mut u8>();
        if array.is_null() {
            return ptr::null_mut();
        }
        *array.add(TLS_INDEX.load(Ordering::Relaxed) as usize)
    }
}

/// Runs the game's TLS callbacks (its array is null-terminated).
///
/// # Safety
/// `game.callbacks` must be null or point to a null-terminated array of callbacks.
unsafe fn run_callbacks(game: &GameTls, reason: u32) {
    if game.callbacks.is_null() {
        return;
    }
    let mut cursor = game.callbacks;
    // SAFETY: the array is null-terminated by contract, so the loop stops at its end.
    unsafe {
        while let Some(callback) = *cursor {
            callback(game.base, reason, ptr::null_mut());
            cursor = cursor.add(1);
        }
    }
}

/// Copies the game's template into the block of a thread whose TEB is `teb`.
///
/// # Safety
/// `teb` must be the TEB of a live thread of this process.
unsafe fn fill_thread_block(teb: *mut c_void, game: &GameTls) {
    // SAFETY: `area` is the block of that thread, `plan.total` bytes long, and `start` the game's template.
    unsafe {
        let area = area_of(teb);
        if !area.is_null() {
            layout::fill(area, game.start, game.plan);
        }
    }
}

/// Fills the block of every other thread of the process (the game does not run on them yet).
///
/// # Safety
/// Called once, from the loader's install call, with `game` valid.
unsafe fn fill_other_threads(game: &GameTls) {
    // SAFETY: Toolhelp snapshot and handles are closed on every path; each TEB comes from the kernel.
    unsafe {
        let snapshot = win::CreateToolhelp32Snapshot(win::TH32CS_SNAPTHREAD, 0);
        if snapshot == win::INVALID_HANDLE_VALUE {
            return;
        }
        let process = win::GetCurrentProcessId();
        let me = win::GetCurrentThreadId();
        let mut entry: win::ThreadEntry32 = core::mem::zeroed();
        entry.size = core::mem::size_of::<win::ThreadEntry32>() as u32;
        let mut more = win::Thread32First(snapshot, &mut entry) != 0;
        while more {
            if entry.owner_process_id == process && entry.thread_id != me {
                fill_thread_by_id(entry.thread_id, game);
            }
            more = win::Thread32Next(snapshot, &mut entry) != 0;
        }
        win::CloseHandle(snapshot);
    }
}

/// # Safety
/// `thread_id` names a thread of this process; `game` valid.
unsafe fn fill_thread_by_id(thread_id: u32, game: &GameTls) {
    // SAFETY: the handle is closed before returning; the TEB pointer comes from NtQueryInformationThread.
    unsafe {
        let thread = win::OpenThread(win::THREAD_QUERY_INFORMATION, 0, thread_id);
        if thread.is_null() {
            return;
        }
        let mut info: win::ThreadBasicInformation = core::mem::zeroed();
        let status = win::NtQueryInformationThread(
            thread,
            win::THREAD_BASIC_INFORMATION_CLASS,
            (&raw mut info).cast(),
            core::mem::size_of::<win::ThreadBasicInformation>() as u32,
            ptr::null_mut(),
        );
        if status >= 0 && !info.teb.is_null() {
            fill_thread_block(info.teb, game);
        }
        win::CloseHandle(thread);
    }
}

/// TLS callback of this DLL (run by the loader for every thread that starts after it).
///
/// # Safety
/// Called by the Windows loader only, with its documented arguments.
pub unsafe extern "system" fn on_thread(_module: *mut c_void, reason: u32, _reserved: *mut c_void) {
    let game = GAME.load(Ordering::SeqCst);
    if game.is_null() {
        return;
    }
    // SAFETY: `game` is either null (checked) or a leaked heap description, never freed.
    let game = unsafe { &*game };
    // SAFETY: runs on the thread concerned, whose TEB is the current one.
    unsafe {
        match reason {
            DLL_THREAD_ATTACH => {
                fill_thread_block(current_teb(), game);
                run_callbacks(game, DLL_THREAD_ATTACH);
            }
            DLL_THREAD_DETACH => run_callbacks(game, DLL_THREAD_DETACH),
            _ => {}
        }
    }
}

/// Stores the game's TLS description in a leaked heap block; `None` when allocation fails.
fn leak_description(game: GameTls) -> Option<*mut GameTls> {
    // SAFETY: the heap is the process heap; the block is initialised before use.
    unsafe {
        let block = win::HeapAlloc(win::GetProcessHeap(), 0, core::mem::size_of::<GameTls>())
            .cast::<GameTls>();
        if block.is_null() {
            return None;
        }
        block.write(game);
        Some(block)
    }
}

/// Installs the game's TLS image: writes the index, fills the current and the other threads, runs
/// the game's `DLL_PROCESS_ATTACH` callbacks. Returns 1 on success, 0 when refused.
///
/// # Safety
/// `base` is the game image's base; `directory` points to its `IMAGE_TLS_DIRECTORY64`, with
/// relocations applied.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AnyXboxX_InstallImageTls(
    base: *mut c_void,
    directory: *const TlsDirectory,
) -> i32 {
    // SAFETY: the caller passes a valid directory (checked for null below).
    unsafe {
        let Some(directory) = directory.as_ref() else {
            return 0;
        };
        if base.is_null() || directory.address_of_index.is_null() {
            return 0;
        }
        let Some(plan) = layout::plan(
            directory.start_of_raw_data as usize,
            directory.end_of_raw_data as usize,
            directory.size_of_zero_fill,
        ) else {
            return 0;
        };
        *directory.address_of_index = TLS_INDEX.load(Ordering::Relaxed);
        if area_of(current_teb()).is_null() {
            return 0;
        }
        let game = GameTls {
            start: directory.start_of_raw_data,
            plan,
            callbacks: directory.address_of_callbacks,
            base,
        };
        let Some(game_ptr) = leak_description(game) else {
            return 0;
        };
        let game = &*game_ptr;
        fill_thread_block(current_teb(), game);
        fill_other_threads(game);
        GAME.store(game_ptr, Ordering::SeqCst);
        run_callbacks(game, DLL_PROCESS_ATTACH);
        1
    }
}

/// Image unloaded: threads that start from now on no longer receive the game's image.
///
/// # Safety
/// `base` is the base of an image previously installed (or any value).
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AnyXboxX_RemoveImageTls(base: *mut c_void) {
    let game = GAME.load(Ordering::SeqCst);
    // SAFETY: a non-null pointer in GAME is a leaked description, valid for the process lifetime.
    if !game.is_null() && unsafe { (*game).base } == base {
        GAME.store(ptr::null_mut(), Ordering::SeqCst);
    }
}
