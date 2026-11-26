//! The DXGI factories the native mode hands the title (`AnyXboxX_CreateDXGIFactory`, `…1`, `…2`).
//! A factory keeps its methods, but its swap-chain creation is patched so that each swap chain's
//! `Present` (and `Present1`) is timed by the frame counter, then presented as before. Port of the
//! export part of `frame_counter.cpp`: the vtables are patched in place, as the C++ does.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows::Win32::Graphics::Dxgi::IDXGISwapChain;
use windows::core::Interface;

use crate::frame_counter;
use crate::guid::Guid;
use crate::object::HResult;

// Vtable slots of the DXGI interfaces (public order of their methods).
const FACTORY_CREATE_SWAP_CHAIN: usize = 10;
const FACTORY2_CREATE_SWAP_CHAIN_FOR_HWND: usize = 15;
const SWAP_CHAIN_PRESENT: usize = 8;
const SWAP_CHAIN1_PRESENT1: usize = 22;
const IUNKNOWN_QUERY_INTERFACE: usize = 0;
const IUNKNOWN_RELEASE: usize = 2;

const IID_FACTORY2: &str = "50C83A1C-E072-4C48-87B0-3630FA36A6D0";
const IID_SWAP_CHAIN1: &str = "790A45F7-0D42-4876-983A-0A55CFE6F4AA";
const PAGE_READWRITE: u32 = 0x04;

#[link(name = "dxgi", kind = "raw-dylib", modifiers = "+verbatim")]
unsafe extern "system" {
    #[link_name = "CreateDXGIFactory"]
    fn dxgi_create_factory(riid: *const Guid, out: *mut *mut c_void) -> HResult;
    #[link_name = "CreateDXGIFactory1"]
    fn dxgi_create_factory1(riid: *const Guid, out: *mut *mut c_void) -> HResult;
    #[link_name = "CreateDXGIFactory2"]
    fn dxgi_create_factory2(flags: u32, riid: *const Guid, out: *mut *mut c_void) -> HResult;
}

#[link(name = "kernel32", kind = "raw-dylib", modifiers = "+verbatim")]
unsafe extern "system" {
    fn VirtualProtect(address: *const c_void, size: usize, new: u32, old: *mut u32) -> i32;
    fn GetCurrentProcess() -> isize;
    fn FlushInstructionCache(process: isize, address: *const c_void, size: usize) -> i32;
}

#[link(name = "user32", kind = "raw-dylib", modifiers = "+verbatim")]
unsafe extern "system" {
    fn GetWindowTextW(window: *mut c_void, text: *mut u16, max: i32) -> i32;
    fn SetWindowTextW(window: *mut c_void, text: *const u16) -> i32;
}

/// The original methods, once patched in (zero until then).
static CREATE_SWAP_CHAIN: AtomicUsize = AtomicUsize::new(0);
static CREATE_SWAP_CHAIN_FOR_HWND: AtomicUsize = AtomicUsize::new(0);
static PRESENT: AtomicUsize = AtomicUsize::new(0);
static PRESENT1: AtomicUsize = AtomicUsize::new(0);

type CreateSwapChainFn =
    unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, *mut *mut c_void) -> HResult;
type CreateSwapChainForHwndFn = unsafe extern "system" fn(
    *mut c_void,
    *mut c_void,
    *mut c_void,
    *const c_void,
    *const c_void,
    *mut c_void,
    *mut *mut c_void,
) -> HResult;
type PresentFn = unsafe extern "system" fn(*mut c_void, u32, u32) -> HResult;
type Present1Fn = unsafe extern "system" fn(*mut c_void, u32, u32, *const c_void) -> HResult;

/// Replaces slot `index` of the vtable of `object`; returns the previous method, or `None` when the
/// slot already holds `replacement` (DXGI shares tables between its objects).
///
/// # Safety
/// `object` is a COM object of a live DXGI interface whose vtable has more than `index` slots.
unsafe fn patch_slot(
    object: *mut c_void,
    index: usize,
    replacement: *const c_void,
) -> Option<*const c_void> {
    // SAFETY: the first word of a COM object is its vtable, per the contract.
    let table = unsafe { *(object as *const *mut *const c_void) };
    // SAFETY: the slot is inside the vtable, per the contract.
    let slot = unsafe { table.add(index) };
    // SAFETY: reads the slot we are about to patch.
    let previous = unsafe { *slot };
    if previous == replacement {
        return None;
    }
    let mut old = 0u32;
    // SAFETY: the vtable page is made writable for the write below, then restored.
    unsafe {
        VirtualProtect(
            slot.cast(),
            size_of::<*const c_void>(),
            PAGE_READWRITE,
            &mut old,
        );
        slot.write(replacement);
        VirtualProtect(slot.cast(), size_of::<*const c_void>(), old, &mut old);
        FlushInstructionCache(GetCurrentProcess(), slot.cast(), size_of::<*const c_void>());
    }
    Some(previous)
}

/// The method at `index` of `object`'s vtable.
///
/// # Safety
/// As [`patch_slot`].
unsafe fn slot(object: *mut c_void, index: usize) -> *const c_void {
    // SAFETY: as above.
    unsafe {
        let table = *(object as *const *const *const c_void);
        *table.add(index)
    }
}

/// Calls `QueryInterface` (slot 0) of a COM object.
///
/// # Safety
/// `object` is a live COM object; `iid` and `out` are valid.
unsafe fn query(object: *mut c_void, iid: &Guid, out: *mut *mut c_void) -> HResult {
    // SAFETY: slot 0 of every COM interface is QueryInterface.
    let method: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult =
        unsafe { std::mem::transmute(slot(object, IUNKNOWN_QUERY_INTERFACE)) };
    // SAFETY: the arguments are as the method expects.
    unsafe { method(object, iid, out) }
}

/// Releases one reference of a COM object (slot 2).
///
/// # Safety
/// `object` is a live COM object whose reference we hold.
unsafe fn release(object: *mut c_void) {
    // SAFETY: slot 2 of every COM interface is Release.
    let method: unsafe extern "system" fn(*mut c_void) -> u32 =
        unsafe { std::mem::transmute(slot(object, IUNKNOWN_RELEASE)) };
    // SAFETY: a reference we hold.
    unsafe { method(object) };
}

/// The title's window of a swap chain: its text, if the chain has an output window.
///
/// # Safety
/// `swap_chain` is a live IDXGISwapChain.
unsafe fn window_text(swap_chain: *mut c_void) -> Option<String> {
    // SAFETY: the pointer is a live swap chain, borrowed for the call.
    let swap_chain = unsafe { IDXGISwapChain::from_raw_borrowed(&swap_chain) }?;
    // SAFETY: `desc` is writable.
    let desc = unsafe { swap_chain.GetDesc() }.ok()?;
    let window = desc.OutputWindow.0;
    if window.is_null() {
        return None;
    }
    let mut buffer = [0u16; 256];
    // SAFETY: the buffer holds the length we pass.
    let length = unsafe { GetWindowTextW(window, buffer.as_mut_ptr(), buffer.len() as i32) };
    Some(String::from_utf16_lossy(&buffer[..length.max(0) as usize]))
}

/// Sets the text of the game window of the first swap chain seen (the frame counter's title).
pub fn set_window_title(text: &str) {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    let window = TITLE_WINDOW.load(Ordering::Relaxed) as *mut c_void;
    if !window.is_null() {
        // SAFETY: the window handle was read from a live swap chain; the text is NUL-terminated.
        unsafe { SetWindowTextW(window, wide.as_ptr()) };
    }
}

/// The window the title is drawn in, as an address (0 until a swap chain is hooked).
static TITLE_WINDOW: AtomicUsize = AtomicUsize::new(0);

/// Remembers the output window of a swap chain for the frame counter's title.
///
/// # Safety
/// `swap_chain` is a live IDXGISwapChain.
unsafe fn remember_window(swap_chain: *mut c_void) {
    // SAFETY: as above.
    if let Some(sc) = unsafe { IDXGISwapChain::from_raw_borrowed(&swap_chain) }
        && let Ok(desc) = unsafe { sc.GetDesc() }
        && TITLE_WINDOW.load(Ordering::Relaxed) == 0
    {
        TITLE_WINDOW.store(desc.OutputWindow.0 as usize, Ordering::Relaxed);
    }
}

/// Present of a patched swap chain: times the frame, then presents as the original does.
unsafe extern "system" fn counted_present(this: *mut c_void, sync: u32, flags: u32) -> HResult {
    // SAFETY: `this` is the swap chain the title presents, per the COM contract.
    frame_counter::frame(|| unsafe { window_text(this) });
    let original: PresentFn = unsafe { std::mem::transmute(PRESENT.load(Ordering::Relaxed)) };
    // SAFETY: the original was patched out of this very vtable.
    unsafe { original(this, sync, flags) }
}

/// Present1 of a patched swap chain.
unsafe extern "system" fn counted_present1(
    this: *mut c_void,
    sync: u32,
    flags: u32,
    parameters: *const c_void,
) -> HResult {
    // SAFETY: as in `counted_present`.
    frame_counter::frame(|| unsafe { window_text(this) });
    let original: Present1Fn = unsafe { std::mem::transmute(PRESENT1.load(Ordering::Relaxed)) };
    // SAFETY: as in `counted_present`.
    unsafe { original(this, sync, flags, parameters) }
}

/// Patches a new swap chain's Present and Present1 (when it has them).
///
/// # Safety
/// `swap_chain` is a live IDXGISwapChain.
unsafe fn hook_swap_chain(swap_chain: *mut c_void) {
    // SAFETY: as above.
    if let Some(previous) = unsafe {
        patch_slot(
            swap_chain,
            SWAP_CHAIN_PRESENT,
            counted_present as *const c_void,
        )
    } {
        PRESENT.store(previous as usize, Ordering::Relaxed);
    }
    let iid = Guid::parse(IID_SWAP_CHAIN1).expect("IDXGISwapChain1 GUID");
    let mut swap_chain1 = std::ptr::null_mut();
    // SAFETY: the output pointer is writable.
    if unsafe { query(swap_chain, &iid, &mut swap_chain1) } >= 0 && !swap_chain1.is_null() {
        // SAFETY: the interface we just obtained is live until released below.
        if let Some(previous) = unsafe {
            patch_slot(
                swap_chain1,
                SWAP_CHAIN1_PRESENT1,
                counted_present1 as *const c_void,
            )
        } {
            PRESENT1.store(previous as usize, Ordering::Relaxed);
        }
        // SAFETY: the reference returned by QueryInterface.
        unsafe { release(swap_chain1) };
    }
    // SAFETY: as above.
    unsafe { remember_window(swap_chain) };
}

/// CreateSwapChain of a patched factory: the original, then the new swap chain is hooked.
unsafe extern "system" fn hooked_create_swap_chain(
    this: *mut c_void,
    device: *mut c_void,
    desc: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    let original: CreateSwapChainFn =
        unsafe { std::mem::transmute(CREATE_SWAP_CHAIN.load(Ordering::Relaxed)) };
    // SAFETY: the original, with the caller's arguments.
    let result = unsafe { original(this, device, desc, out) };
    // SAFETY: the output is the title's, written by the original on success.
    if result >= 0
        && let Some(swap_chain) = unsafe { out.as_ref() }
        && !swap_chain.is_null()
    {
        unsafe { hook_swap_chain(*swap_chain) };
    }
    result
}

/// CreateSwapChainForHwnd of a patched factory (IDXGIFactory2).
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn hooked_create_swap_chain_for_hwnd(
    this: *mut c_void,
    device: *mut c_void,
    window: *mut c_void,
    desc: *const c_void,
    fullscreen: *const c_void,
    output: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    let original: CreateSwapChainForHwndFn =
        unsafe { std::mem::transmute(CREATE_SWAP_CHAIN_FOR_HWND.load(Ordering::Relaxed)) };
    // SAFETY: the original, with the caller's arguments.
    let result = unsafe { original(this, device, window, desc, fullscreen, output, out) };
    // SAFETY: as in `hooked_create_swap_chain`.
    if result >= 0
        && let Some(swap_chain) = unsafe { out.as_ref() }
        && !swap_chain.is_null()
    {
        unsafe { hook_swap_chain(*swap_chain) };
    }
    result
}

/// Patches a factory's swap-chain creation (and IDXGIFactory2's, when it has it).
///
/// # Safety
/// `factory` is a live IDXGIFactory.
unsafe fn hook_factory(factory: *mut c_void) {
    // SAFETY: as above.
    if let Some(previous) = unsafe {
        patch_slot(
            factory,
            FACTORY_CREATE_SWAP_CHAIN,
            hooked_create_swap_chain as *const c_void,
        )
    } {
        CREATE_SWAP_CHAIN.store(previous as usize, Ordering::Relaxed);
    }
    let iid = Guid::parse(IID_FACTORY2).expect("IDXGIFactory2 GUID");
    let mut factory2 = std::ptr::null_mut();
    // SAFETY: the output pointer is writable.
    if unsafe { query(factory, &iid, &mut factory2) } >= 0 && !factory2.is_null() {
        // SAFETY: the interface we just obtained is live until released below.
        if let Some(previous) = unsafe {
            patch_slot(
                factory2,
                FACTORY2_CREATE_SWAP_CHAIN_FOR_HWND,
                hooked_create_swap_chain_for_hwnd as *const c_void,
            )
        } {
            CREATE_SWAP_CHAIN_FOR_HWND.store(previous as usize, Ordering::Relaxed);
        }
        // SAFETY: the reference returned by QueryInterface.
        unsafe { release(factory2) };
    }
}

/// Hooks a created factory when the call succeeded.
///
/// # Safety
/// `out` is the output pointer of a DXGI factory creation.
unsafe fn after_create(result: HResult, out: *mut *mut c_void) -> HResult {
    // SAFETY: the output is the title's; written by the creation on success.
    if result >= 0
        && let Some(factory) = unsafe { out.as_ref() }
        && !factory.is_null()
    {
        // SAFETY: a live factory the creation returned.
        unsafe { hook_factory(*factory) };
    }
    result
}

/// `AnyXboxX_CreateDXGIFactory`: `CreateDXGIFactory` with the factory's swap chains counted.
///
/// # Safety
/// As `CreateDXGIFactory`: `riid` is a GUID, `out` a writable pointer.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AnyXboxX_CreateDXGIFactory(
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    // SAFETY: forwarded contract.
    unsafe { after_create(dxgi_create_factory(riid, out), out) }
}

/// `AnyXboxX_CreateDXGIFactory1`.
///
/// # Safety
/// As [`AnyXboxX_CreateDXGIFactory`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AnyXboxX_CreateDXGIFactory1(
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    // SAFETY: forwarded contract.
    unsafe { after_create(dxgi_create_factory1(riid, out), out) }
}

/// `AnyXboxX_CreateDXGIFactory2`.
///
/// # Safety
/// As [`AnyXboxX_CreateDXGIFactory`].
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AnyXboxX_CreateDXGIFactory2(
    flags: u32,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    // SAFETY: forwarded contract.
    unsafe { after_create(dxgi_create_factory2(flags, riid, out), out) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::S_OK;

    #[test]
    fn the_factory_interface_ids_are_the_public_ones() {
        assert_eq!(Guid::parse(IID_FACTORY2).unwrap().data1, 0x50C8_3A1C);
        assert_eq!(Guid::parse(IID_SWAP_CHAIN1).unwrap().data1, 0x790A_45F7);
    }

    #[test]
    fn a_null_output_is_left_alone() {
        // SAFETY: no output pointer means nothing is hooked; the result is passed through.
        assert_eq!(unsafe { after_create(S_OK, std::ptr::null_mut()) }, S_OK);
    }
}
