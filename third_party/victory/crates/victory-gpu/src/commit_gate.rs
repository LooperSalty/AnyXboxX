//! Under low commit, texture uploads of a pass stop before the driver is
//! asked for memory it would refuse, so the game's own allocations and the
//! submissions in flight keep the rest (cool_fox `ab75ac3`, `residency.rs`).
//! Off unless `VICTORY_COMMIT_GATE=1`.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

const GIB: u64 = 1 << 30;

/// The commit (bytes) an upload leaves to the game and its submissions.
pub(crate) const UPLOAD_RESERVE: u64 = 4 * GIB;

/// How long a reading of the available commit is kept, in milliseconds.
const SAMPLE_MS: u64 = 32;

/// When the commit is read again, in milliseconds since first use.
static NEXT_SAMPLE: AtomicU64 = AtomicU64::new(0);
/// The last reading of the available commit, in bytes.
static AVAILABLE: AtomicU64 = AtomicU64::new(u64::MAX);

/// Whether `commit` bytes available leave room for an upload.
pub(crate) fn has_reserve(commit: u64) -> bool {
    commit >= UPLOAD_RESERVE
}

/// Whether the gate is on (`VICTORY_COMMIT_GATE=1`).
fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_COMMIT_GATE").is_some_and(|v| v == "1"))
}

/// The commit still available to the PC, in bytes.
#[cfg(windows)]
fn available_commit() -> u64 {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: plain data the call fills in.
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: a sized structure.
    unsafe { GlobalMemoryStatusEx(&mut m) };
    m.ullAvailPageFile
}

/// Whether a texture upload may go ahead now. Always true with the gate off.
#[cfg(windows)]
pub(crate) fn admit() -> bool {
    if !on() {
        return true;
    }
    let now = crate::host::now_ms();
    if now >= NEXT_SAMPLE.load(Ordering::Relaxed) {
        NEXT_SAMPLE.store(now.saturating_add(SAMPLE_MS), Ordering::Relaxed);
        AVAILABLE.store(available_commit(), Ordering::Relaxed);
    }
    has_reserve(AVAILABLE.load(Ordering::Relaxed))
}

#[cfg(not(windows))]
pub(crate) fn admit() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_upload_needs_the_whole_reserve_free() {
        assert!(!has_reserve(0));
        assert!(!has_reserve(UPLOAD_RESERVE - 1));
        assert!(has_reserve(UPLOAD_RESERVE));
    }

    #[test]
    fn the_gate_is_off_unless_asked_for() {
        assert!(admit());
    }
}
