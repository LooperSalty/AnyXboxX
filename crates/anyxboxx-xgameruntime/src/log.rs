//! The runtime's journal, on stderr, shared with AnyXboxX (same channel as the C++ `anyxboxx::Log`).
//!
//! Messages of the title (XErrorReport) are always written; the details of the calls only with
//! `ANYXBOXX_TRACE=1`.

use std::io::Write;
use std::sync::OnceLock;

/// Whether `ANYXBOXX_TRACE=1` is set (read once, as the C++ `TraceEnabled`).
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("ANYXBOXX_TRACE").is_ok_and(|value| value == "1"))
}

/// Writes one line to stderr. A closed stderr must not crash the title: the line is then lost.
pub fn write(args: std::fmt::Arguments<'_>) {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{args}");
    let _ = stderr.flush();
}

/// A detail line, written only with `ANYXBOXX_TRACE=1`.
macro_rules! trace {
    ($($arg:tt)*) => {
        if $crate::log::enabled() {
            $crate::log::write(format_args!($($arg)*));
        }
    };
}
pub(crate) use trace;
