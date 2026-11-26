//! anyxboxx_tls.dll in Rust: the thread-local carrier of a game loaded by hand (port of
//! `runtime/tls/anyxboxx_tls.cpp`, same two exports, checked by `tests/test_native_tls*.py`
//! against this DLL through `tests/test_rust_dlls.py`).
//!
//! `no_std` in the DLL build: Rust std would add its own thread-locals to this DLL's TLS block,
//! which the game's template would overwrite. Under `cargo test` std is linked for the unit tests
//! of the pure logic (`layout`), and the Windows glue is left out.

#![cfg_attr(not(test), no_std)]

mod layout;

#[cfg(not(test))]
mod install;
#[cfg(not(test))]
mod tls_image;
#[cfg(not(test))]
mod win;

/// The DLL cannot unwind (no_std, no CRT): a panic stops the process at the faulting instruction.
#[cfg(not(test))]
#[panic_handler]
fn on_panic(_info: &core::panic::PanicInfo) -> ! {
    // SAFETY: `ud2` raises an invalid-opcode exception and never returns.
    unsafe { core::arch::asm!("ud2", options(noreturn)) }
}
