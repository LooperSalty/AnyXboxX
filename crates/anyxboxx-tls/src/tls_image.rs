//! The DLL's own TLS directory, written by hand (no CRT, no `thread_local!`, stable Rust).
//!
//! - `CARRIER` lives in the `.tls` section: the linker makes it the template of this DLL's TLS
//!   block, so every thread gets a block of `CARRIER_BYTES` and the game's template is copied at
//!   its offset 0. It is the only TLS data of the DLL.
//! - `TLS_INDEX` receives the index that Windows gives this DLL: the loader writes it at the
//!   address the directory gives (`AddressOfIndex`), whatever the symbol is called.
//! - `_tls_used` is the TLS directory (`IMAGE_TLS_DIRECTORY64`): the MSVC linker finds it by name.
//!   It is written in assembly on purpose: rustc exports every `#[export_name]` static, and the
//!   DLL's export table must hold only the two install/remove functions.
//! - `CALLBACKS` is the null-terminated callback array of that directory.

use crate::install::on_thread;
use crate::layout::CARRIER_BYTES;
use core::cell::UnsafeCell;
use core::sync::atomic::AtomicU32;

/// Raw bytes of the carrier. Written only by the game, through its thread's TLS slot.
/// The 16-byte alignment is the one of the C++ DLL's TLS directory.
#[repr(C, align(16))]
pub struct Carrier(UnsafeCell<[u8; CARRIER_BYTES]>);

// SAFETY: the Rust side never touches the bytes; threads access them through their own blocks.
unsafe impl Sync for Carrier {}

#[unsafe(link_section = ".tls")]
static CARRIER: Carrier = Carrier(UnsafeCell::new([0; CARRIER_BYTES]));

/// Index of this DLL in every thread's `ThreadLocalStoragePointer`, written by the loader.
pub(crate) static TLS_INDEX: AtomicU32 = AtomicU32::new(0);

/// `IMAGE_TLS_DIRECTORY64`: 4 pointers, then SizeOfZeroFill and Characteristics (40 bytes).
#[repr(C)]
pub struct TlsDirectory {
    pub start_of_raw_data: *const u8,
    pub end_of_raw_data: *const u8,
    pub address_of_index: *mut u32,
    pub address_of_callbacks: *const Option<TlsCallback>,
    pub size_of_zero_fill: u32,
    pub characteristics: u32,
}

/// `PIMAGE_TLS_CALLBACK`.
pub type TlsCallback = unsafe extern "system" fn(
    module: *mut core::ffi::c_void,
    reason: u32,
    reserved: *mut core::ffi::c_void,
);

static CALLBACKS: [Option<TlsCallback>; 2] = [Some(on_thread), None];

// The directory itself: `_tls_used` in `.rdata`, 40 bytes, 16-byte aligned like the C++ one.
core::arch::global_asm!(
    ".section .rdata,\"dr\"",
    ".balign 8",
    ".globl _tls_used",
    "_tls_used:",
    ".quad {carrier}",
    ".quad {carrier} + {carrier_bytes}",
    ".quad {index}",
    ".quad {callbacks}",
    ".long 0",
    ".long 0x500000",
    ".text",
    carrier = sym CARRIER,
    carrier_bytes = const CARRIER_BYTES,
    index = sym TLS_INDEX,
    callbacks = sym CALLBACKS,
);
