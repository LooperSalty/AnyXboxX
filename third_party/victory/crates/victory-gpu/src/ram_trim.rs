//! Opt-in host RAM trimming policy and private snapshot backing storage.
//!
//! Automatic thresholds are available only with `VICTORY_RAM_TRIM=1` (also
//! `true` or `on`). The storage operations are explicit and independent of the
//! policy. No consumer is enabled by exposing this module.

mod metrics;
mod policy;
mod snapshot;
mod vm;
#[cfg(windows)]
mod vm_windows;

pub use metrics::{COUNTERS, RamTrimCounters, note_lists_trimmed, note_pool_trimmed};
#[cfg(any(feature = "diagnostics", test))]
pub use metrics::{RamTrimSnapshot, diagnostics_line, snapshot};
pub use policy::{
    HostRamStatus, HostTier, TrimPolicy, TrimThresholds, enabled, host_status, host_status_cached,
    tier_for_total_bytes,
};
pub use snapshot::{SnapshotBuffer, is_all_zero, maybe_shrink_scratch_vec};
pub use vm::{VmBlock, verify_safe_for_decommit};

pub const KIB: usize = 1024;
pub const MIB: usize = 1024 * KIB;
pub const GIB: u64 = 1024 * 1024 * 1024;

/// Piece size for Windows snapshot reservations, in bytes.
pub const VM_PIECE: usize = 64 * KIB;
/// Minimum snapshot length for selecting a VM block instead of a small heap buffer.
pub const VM_THRESHOLD: usize = 256 * KIB;

#[cfg(test)]
mod tests;
