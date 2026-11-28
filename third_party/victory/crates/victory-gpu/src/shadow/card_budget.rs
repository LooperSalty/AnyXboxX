//! Arithmetic for the card-buffer admission and streaming reserve policies.

pub(crate) const LARGE: u64 = 1 << 20;

pub(crate) fn opt_in(value: Option<&str>) -> bool {
    value.is_some_and(|v| {
        ["1", "true", "on"]
            .iter()
            .any(|yes| v.trim().eq_ignore_ascii_case(yes))
    })
}

pub(crate) fn large_reserve(least: u64, most: u64, override_mb: Option<u64>) -> u64 {
    if least >= LARGE {
        return 0;
    }
    override_mb.map_or((256 << 20).min(most / 3), |mb| {
        mb.saturating_mul(LARGE).min(most)
    })
}

fn percent(bytes: u64, pct: u64) -> u64 {
    (u128::from(bytes) * u128::from(pct) / 100) as u64
}

pub(crate) fn pressure_target(memory: Option<[u64; 4]>, small: u64, pct: u64) -> u64 {
    match memory {
        Some([budget, used, ..]) if budget > 0 && used >= percent(budget, pct) => used
            .saturating_sub(percent(budget, pct.saturating_sub(4)))
            .min(small),
        _ => 0,
    }
}

#[derive(Default)]
pub(crate) struct Budget {
    active: u64,
    pub(crate) small: u64,
    /// Room lent to streaming allocations under pressure. It transfers back
    /// to the buffer budget when a new local buffer actually takes room.
    released: u64,
}

impl Budget {
    pub(crate) const fn new() -> Self {
        Self {
            active: 0,
            small: 0,
            released: 0,
        }
    }

    pub(crate) fn reserved_for_large(&self, size: u64, most: u64, reserve: u64) -> bool {
        size < LARGE && self.small.saturating_add(size) > most.saturating_sub(reserve)
    }

    /// Minimum to reclaim under both the live-byte cap and the DXGI budget.
    pub(crate) fn shortfall(
        &self,
        size: u64,
        most: u64,
        memory: Option<[u64; 4]>,
        limit: u64,
    ) -> (u64, u64) {
        let dxgi = match memory {
            Some([budget, used, ..]) if budget > 0 => used
                .saturating_add(size)
                .saturating_sub(percent(budget, limit)),
            _ => 0,
        };
        let cap = size.saturating_sub(most.saturating_sub(self.active));
        (cap.max(dxgi), dxgi)
    }

    pub(crate) fn admit(&mut self, size: u64) {
        self.active += size;
        if size < LARGE {
            self.small += size;
        }
        self.released = self.released.saturating_sub(size);
    }

    pub(crate) fn release(&mut self, size: u64, pressure: bool) {
        self.active -= size;
        if size < LARGE {
            self.small -= size;
        }
        if pressure {
            self.released += size;
        }
    }

    pub(crate) fn unreserved(&self) -> u64 {
        self.active + self.released
    }
}

pub(crate) fn remaining_card_room(
    buffers: u64,
    used: u64,
    refused: u64,
    windows: u64,
    window_used: u64,
) -> u64 {
    buffers
        .saturating_sub(used.saturating_add(refused))
        .saturating_add(windows.saturating_sub(window_used))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demotion_is_default_off() {
        for value in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("off"),
            Some("yes"),
        ] {
            assert!(!opt_in(value), "{value:?}");
        }
        for value in ["1", "true", "on", " TRUE "] {
            assert!(opt_in(Some(value)), "{value:?}");
        }
    }

    #[test]
    fn pressure_demotion_then_reuse_restores_card_room() {
        let mut budget = Budget::new();
        let size = 64 << 10;
        let room =
            |budget: &Budget| remaining_card_room(8 * LARGE, budget.unreserved(), LARGE, 0, 0);
        budget.admit(size);
        let baseline = room(&budget);
        for _ in 0..8 {
            budget.release(size, true);
            assert_eq!(budget.active, 0);
            assert_eq!(
                room(&budget),
                baseline,
                "released VRAM is available to streaming"
            );
            budget.admit(size / 2);
            assert_eq!(budget.released, size / 2);
            budget.admit(size / 2);
            assert_eq!(budget.active, size);
            assert_eq!(budget.released, 0);
            assert_eq!(room(&budget), baseline);
        }
        budget.release(size / 2, false);
        budget.release(size / 2, false);
        assert_eq!(room(&budget), baseline + size);
    }

    #[test]
    fn large_headroom_and_pressure_hysteresis_are_bounded() {
        assert_eq!(large_reserve(LARGE, 512 * LARGE, None), 0);
        assert_eq!(large_reserve(LARGE / 2, 512 * LARGE, None), 512 * LARGE / 3);
        assert_eq!(large_reserve(LARGE / 2, LARGE, Some(0)), 0);
        assert_eq!(large_reserve(LARGE / 2, LARGE, Some(u64::MAX)), LARGE);
        for budget in [8 << 30, 12 << 30] {
            assert_eq!(
                pressure_target(Some([budget, percent(budget, 87), 0, 0]), LARGE, 88),
                0
            );
            assert_eq!(
                pressure_target(Some([budget, percent(budget, 88), 0, 0]), LARGE, 88),
                LARGE
            );
        }
        let mut budget = Budget::new();
        let cap = LARGE * 3;
        let small = LARGE / 2;
        for _ in 0..4 {
            assert!(!budget.reserved_for_large(small, cap, LARGE));
            budget.admit(small);
        }
        assert!(budget.reserved_for_large(small, cap, LARGE));
        assert!(!budget.reserved_for_large(LARGE, cap, LARGE));
        assert_eq!(budget.shortfall(LARGE, cap, None, 97), (0, 0));
        assert_eq!(budget.shortfall(2 * LARGE, cap, None, 97), (LARGE, 0));
        assert_eq!(
            budget.shortfall(LARGE, cap, Some([100 * LARGE, 97 * LARGE, 0, 0]), 97),
            (LARGE, LARGE)
        );
    }
}
