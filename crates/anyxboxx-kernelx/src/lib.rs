//! kernelx.dll in Rust: the Xbox kernel, translated to the PC's.
//!
//! Console binaries import kernelx.dll instead of kernel32.dll. Every function the two share is
//! forwarded to Windows' kernel32/kernelbase by the linker (`build.rs` turns the table of
//! `tools/kernelx_exports.py` into an export file). Console-only functions will be added here as
//! the reports reveal them. Port of the former C++ `runtime/kernelx`, whose tests
//! (`tests/test_kernelx.py`) and export table it reproduced before the C++ DLL was deleted.

/// Number of forwarders exported, from the generated table.
pub const FORWARDERS: usize = {
    let text = env!("KERNELX_FORWARDERS").as_bytes();
    let mut value = 0;
    let mut index = 0;
    while index < text.len() {
        value = value * 10 + (text[index] - b'0') as usize;
        index += 1;
    }
    value
};

// tools/kernelx_exports.py lists about 2,600 functions of kernel32 and kernelbase: a much
// smaller table means the generated file was truncated.
const _: () = assert!(FORWARDERS > 2000);
