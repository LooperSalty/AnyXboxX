//! Runtime log on stderr, shared with AnyXboxX (the C++ `anyxboxx::Log` and `ANYXBOXX_TRACE` of
//! `runtime/common/trace.h`): messages always, call details only with `ANYXBOXX_TRACE=1`.

use std::io::Write;
use std::sync::LazyLock;

/// Writes one line. A closed stderr must not crash the title: the line is then lost.
pub(crate) fn write_line(message: std::fmt::Arguments<'_>) {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{message}");
    let _ = stderr.flush();
}

/// Whether call details are logged (`ANYXBOXX_TRACE` is exactly `1`), read once.
pub(crate) fn trace_enabled() -> bool {
    static ENABLED: LazyLock<bool> =
        LazyLock::new(|| std::env::var_os("ANYXBOXX_TRACE").is_some_and(|value| value == "1"));
    *ENABLED
}

/// `log!` always writes; `trace!` only when [`trace_enabled`].
macro_rules! log {
    ($($argument:tt)*) => { $crate::log::write_line(format_args!($($argument)*)) };
}

macro_rules! trace {
    ($($argument:tt)*) => {
        if $crate::log::trace_enabled() {
            $crate::log::write_line(format_args!($($argument)*));
        }
    };
}

pub(crate) use {log, trace};
