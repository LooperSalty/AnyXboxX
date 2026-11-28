//! Host memory status and opt-in thresholds for future trimming consumers.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::{GIB, MIB};

/// Host physical RAM capacity class.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HostTier {
    Tight16,
    #[default]
    Standard32,
    Large,
}

impl HostTier {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Tight16 => "16 GiB class",
            Self::Standard32 => "32 GiB class",
            Self::Large => "above 32 GiB class",
        }
    }
}

/// Host memory and current process memory in bytes. Zero means unavailable or zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HostRamStatus {
    pub total_phys: u64,
    pub avail_phys: u64,
    pub avail_commit: u64,
    pub process_working_set: u64,
    pub process_private_bytes: u64,
}

/// Reads the process-wide opt-in once. No other value enables automatic trims.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("VICTORY_RAM_TRIM").is_ok_and(|value| {
            let value = value.trim();
            value == "1" || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("on")
        })
    })
}

/// Classifies nominal 16 and 32 GiB hosts with 1 GiB of tolerance. Unknown hosts
/// use the middle tier.
pub const fn tier_for_total_bytes(total_phys: u64) -> HostTier {
    if total_phys != 0 && total_phys <= 17 * GIB {
        HostTier::Tight16
    } else if total_phys <= 33 * GIB {
        HostTier::Standard32
    } else {
        HostTier::Large
    }
}

/// Queries the OS. Hosts without a Windows memory status API report unknown values.
#[cfg(windows)]
pub fn host_status() -> HostRamStatus {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut mem = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    let mut process = PROCESS_MEMORY_COUNTERS {
        cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    // SAFETY: both output structures are writable and set to the size required by Win32.
    let mem_ok = unsafe { GlobalMemoryStatusEx(&mut mem) } != 0;
    let process_ok =
        unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut process, process.cb) } != 0;

    let mut status = HostRamStatus::default();
    if mem_ok {
        status.total_phys = mem.ullTotalPhys;
        status.avail_phys = mem.ullAvailPhys;
        status.avail_commit = mem.ullAvailPageFile;
    }
    if process_ok {
        status.process_working_set = process.WorkingSetSize as u64;
        status.process_private_bytes = process.PagefileUsage as u64;
    }
    status
}

#[cfg(not(windows))]
pub fn host_status() -> HostRamStatus {
    HostRamStatus::default()
}

/// Returns host RAM status from a cache with a 250 ms lifetime.
pub fn host_status_cached() -> HostRamStatus {
    const TTL: Duration = Duration::from_millis(250);
    static CACHE: Mutex<Option<(Instant, HostRamStatus)>> = Mutex::new(None);

    let mut cache = CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((at, status)) = *cache
        && at.elapsed() < TTL
    {
        return status;
    }
    let status = host_status();
    *cache = Some((Instant::now(), status));
    status
}

/// Automatic trimming is disabled unless thresholds are explicitly enabled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrimPolicy {
    pub tier: HostTier,
    thresholds: Option<TrimThresholds>,
}

/// Candidate thresholds for consumers, only supplied by an enabled policy.
/// The foundation does not apply these thresholds itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrimThresholds {
    pub low_memory: bool,
    pub critical_memory: bool,
    pub pool_idle_ms: u64,
    pub shadow_pool_warm_bytes: u64,
    pub shadow_pool_reserve_slots: usize,
    pub scratch_spare_warm_bytes: u64,
    pub staging_pool_warm_blocks: usize,
    pub list_idle_warm_count: usize,
    pub list_idle_ms: u64,
    pub shader_dxil_idle_ms: u64,
    pub shader_container_idle_ms: u64,
    pub decommit_cold_reference_ms: Option<u64>,
    pub fetched_warm_bytes: u64,
}

impl TrimPolicy {
    /// Builds a policy from explicit host parameters. `false` suppresses every
    /// automatic threshold, even under critical pressure.
    pub fn for_host(is_enabled: bool, total_phys: u64, avail_phys: u64, avail_commit: u64) -> Self {
        let tier = tier_for_total_bytes(total_phys);
        if !is_enabled {
            return Self {
                tier,
                thresholds: None,
            };
        }
        Self::opt_in(HostRamStatus {
            total_phys,
            avail_phys,
            avail_commit,
            ..HostRamStatus::default()
        })
    }

    /// Builds the enabled thresholds for a supplied status, without reading the environment.
    pub fn opt_in(status: HostRamStatus) -> Self {
        let tier = tier_for_total_bytes(status.total_phys);
        let under = |limit| {
            [status.avail_phys, status.avail_commit]
                .into_iter()
                .any(|available| available > 0 && available < limit)
        };
        let low_memory = under(4 * GIB);
        let critical_memory = under(2 * GIB);
        const MIB_BYTES: u64 = MIB as u64;

        let thresholds = if critical_memory {
            TrimThresholds {
                low_memory,
                critical_memory,
                pool_idle_ms: 1_000,
                shadow_pool_warm_bytes: 0,
                shadow_pool_reserve_slots: 8,
                scratch_spare_warm_bytes: 0,
                staging_pool_warm_blocks: 0,
                list_idle_warm_count: 16,
                list_idle_ms: 2_000,
                shader_dxil_idle_ms: 0,
                shader_container_idle_ms: 0,
                decommit_cold_reference_ms: Some(1_000),
                fetched_warm_bytes: 4 * MIB_BYTES,
            }
        } else {
            match tier {
                HostTier::Tight16 => TrimThresholds {
                    low_memory,
                    critical_memory,
                    pool_idle_ms: 2_000,
                    shadow_pool_warm_bytes: 0,
                    shadow_pool_reserve_slots: 8,
                    scratch_spare_warm_bytes: 0,
                    staging_pool_warm_blocks: 0,
                    list_idle_warm_count: 24,
                    list_idle_ms: 3_000,
                    shader_dxil_idle_ms: 0,
                    shader_container_idle_ms: 1_000,
                    decommit_cold_reference_ms: Some(2_000),
                    fetched_warm_bytes: 4 * MIB_BYTES,
                },
                HostTier::Standard32 => {
                    let pool_idle_ms = if low_memory { 2_000 } else { 4_000 };
                    TrimThresholds {
                        low_memory,
                        critical_memory,
                        pool_idle_ms,
                        shadow_pool_warm_bytes: if low_memory { 0 } else { 32 * MIB_BYTES },
                        shadow_pool_reserve_slots: 8,
                        scratch_spare_warm_bytes: if low_memory { 0 } else { 8 * MIB_BYTES },
                        staging_pool_warm_blocks: usize::from(!low_memory),
                        list_idle_warm_count: if low_memory { 24 } else { 48 },
                        list_idle_ms: if low_memory { 3_000 } else { 5_000 },
                        shader_dxil_idle_ms: 0,
                        shader_container_idle_ms: 2_000,
                        decommit_cold_reference_ms: Some(pool_idle_ms),
                        fetched_warm_bytes: 8 * MIB_BYTES,
                    }
                }
                HostTier::Large => TrimThresholds {
                    low_memory,
                    critical_memory,
                    pool_idle_ms: if low_memory { 2_500 } else { 6_000 },
                    shadow_pool_warm_bytes: if low_memory { 16 } else { 64 } * MIB_BYTES,
                    shadow_pool_reserve_slots: 12,
                    scratch_spare_warm_bytes: if low_memory { 8 } else { 16 } * MIB_BYTES,
                    staging_pool_warm_blocks: if low_memory { 1 } else { 2 },
                    list_idle_warm_count: if low_memory { 32 } else { 64 },
                    list_idle_ms: 6_000,
                    shader_dxil_idle_ms: 0,
                    shader_container_idle_ms: 4_000,
                    decommit_cold_reference_ms: Some(6_000),
                    fetched_warm_bytes: 16 * MIB_BYTES,
                },
            }
        };
        Self {
            tier,
            thresholds: Some(thresholds),
        }
    }

    /// Returns a disabled policy without querying the host when not opted in.
    pub fn cached() -> Self {
        if enabled() {
            Self::opt_in(host_status_cached())
        } else {
            Self::default()
        }
    }

    /// `None` means no automatic aging, pressure trimming, or decommit applies.
    pub const fn thresholds(&self) -> Option<&TrimThresholds> {
        self.thresholds.as_ref()
    }
}
