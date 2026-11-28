//! Counters for snapshot backing and future RAM trim consumers.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Relaxed diagnostic counters. The reserved and committed fields describe VM
/// reservations only; they do not include heap copies of preserved bytes.
pub struct RamTrimCounters {
    pub vm_reserved_bytes: AtomicU64,
    pub vm_committed_bytes: AtomicU64,
    pub vm_decommitted_bytes: AtomicU64,
    pub vm_zero_decommitted_bytes: AtomicU64,
    pub vm_recommitted_bytes: AtomicU64,
    pub pool_trimmed_bytes: AtomicU64,
    pub list_trimmed_count: AtomicU64,
    pub shader_dxil_dropped_bytes: AtomicU64,
    pub shader_dxil_reloaded_bytes: AtomicU64,
    pub shader_container_orig_bytes: AtomicU64,
    pub shader_container_retained_bytes: AtomicU64,
    pub unsafe_decommit_blocked: AtomicU64,
}

pub static COUNTERS: RamTrimCounters = RamTrimCounters {
    vm_reserved_bytes: AtomicU64::new(0),
    vm_committed_bytes: AtomicU64::new(0),
    vm_decommitted_bytes: AtomicU64::new(0),
    vm_zero_decommitted_bytes: AtomicU64::new(0),
    vm_recommitted_bytes: AtomicU64::new(0),
    pool_trimmed_bytes: AtomicU64::new(0),
    list_trimmed_count: AtomicU64::new(0),
    shader_dxil_dropped_bytes: AtomicU64::new(0),
    shader_dxil_reloaded_bytes: AtomicU64::new(0),
    shader_container_orig_bytes: AtomicU64::new(0),
    shader_container_retained_bytes: AtomicU64::new(0),
    unsafe_decommit_blocked: AtomicU64::new(0),
};

/// Records bytes explicitly released from idle pools or scratch storage.
pub fn note_pool_trimmed(bytes: u64) {
    COUNTERS.pool_trimmed_bytes.fetch_add(bytes, Relaxed);
}

/// Records the number of idle command lists released.
pub fn note_lists_trimmed(count: u64) {
    COUNTERS.list_trimmed_count.fetch_add(count, Relaxed);
}

#[cfg(any(feature = "diagnostics", test))]
use super::policy::{HostRamStatus, HostTier, enabled, host_status, tier_for_total_bytes};

/// Point-in-time diagnostic readings, not a synchronized view across all counters.
#[cfg(any(feature = "diagnostics", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RamTrimSnapshot {
    pub enabled: bool,
    pub tier: HostTier,
    pub host: HostRamStatus,
    pub vm_reserved_bytes: u64,
    pub vm_committed_bytes: u64,
    pub vm_decommitted_bytes: u64,
    pub vm_zero_decommitted_bytes: u64,
    pub vm_recommitted_bytes: u64,
    pub pool_trimmed_bytes: u64,
    pub list_trimmed_count: u64,
    pub shader_dxil_dropped_bytes: u64,
    pub shader_dxil_reloaded_bytes: u64,
    pub shader_container_saved_bytes: u64,
    pub unsafe_decommit_blocked: u64,
}

/// Captures current counters and host RAM status together for diagnostics.
#[cfg(any(feature = "diagnostics", test))]
pub fn snapshot() -> RamTrimSnapshot {
    let host = host_status();
    let original = COUNTERS.shader_container_orig_bytes.load(Relaxed);
    let retained = COUNTERS.shader_container_retained_bytes.load(Relaxed);
    RamTrimSnapshot {
        enabled: enabled(),
        tier: tier_for_total_bytes(host.total_phys),
        host,
        vm_reserved_bytes: COUNTERS.vm_reserved_bytes.load(Relaxed),
        vm_committed_bytes: COUNTERS.vm_committed_bytes.load(Relaxed),
        vm_decommitted_bytes: COUNTERS.vm_decommitted_bytes.load(Relaxed),
        vm_zero_decommitted_bytes: COUNTERS.vm_zero_decommitted_bytes.load(Relaxed),
        vm_recommitted_bytes: COUNTERS.vm_recommitted_bytes.load(Relaxed),
        pool_trimmed_bytes: COUNTERS.pool_trimmed_bytes.load(Relaxed),
        list_trimmed_count: COUNTERS.list_trimmed_count.load(Relaxed),
        shader_dxil_dropped_bytes: COUNTERS.shader_dxil_dropped_bytes.load(Relaxed),
        shader_dxil_reloaded_bytes: COUNTERS.shader_dxil_reloaded_bytes.load(Relaxed),
        shader_container_saved_bytes: original.saturating_sub(retained),
        unsafe_decommit_blocked: COUNTERS.unsafe_decommit_blocked.load(Relaxed),
    }
}

/// Formats the counters for a diagnostics log. VM decommit is not net RAM saved.
#[cfg(any(feature = "diagnostics", test))]
pub fn diagnostics_line() -> String {
    let s = snapshot();
    let mib = |bytes| bytes as f64 / super::MIB as f64;
    let mode = if s.enabled { "enabled" } else { "disabled" };
    format!(
        "ram trim ({}, {mode}): VM reserved {:.1} MiB / committed {:.1} MiB, \
         decommitted {:.1} MiB (zero {:.1} MiB, recommitted {:.1} MiB), \
         pools trimmed {:.1} MiB ({} lists), DXIL dropped {:.1} MiB (reloaded {:.1} MiB), \
         shader containers saved {:.1} MiB, blocked decommits {}",
        s.tier.label(),
        mib(s.vm_reserved_bytes),
        mib(s.vm_committed_bytes),
        mib(s.vm_decommitted_bytes),
        mib(s.vm_zero_decommitted_bytes),
        mib(s.vm_recommitted_bytes),
        mib(s.pool_trimmed_bytes),
        s.list_trimmed_count,
        mib(s.shader_dxil_dropped_bytes),
        mib(s.shader_dxil_reloaded_bytes),
        mib(s.shader_container_saved_bytes),
        s.unsafe_decommit_blocked,
    )
}
