//! Pure logic of the carrier: sizes of the game's TLS template and the copy into a thread's block.
//! No Windows call here, so it is unit-tested under std (`cargo test`).

/// Most TLS bytes the game may use: its template plus its zero fill (256 KiB, as the C++ DLL).
pub const CAPACITY: usize = 256 * 1024;
/// Size of the carrier: the capacity plus 64 bytes of margin, the whole TLS block of this DLL.
pub const CARRIER_BYTES: usize = CAPACITY + 64;

/// Bytes of the game's template (`data`) and of its whole block (`total` = data + zero fill).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub data: usize,
    pub total: usize,
}

/// Checks a game's `IMAGE_TLS_DIRECTORY64` range; `None` when it is inverted or larger than the carrier.
pub fn plan(start: usize, end: usize, zero_fill: u32) -> Option<Plan> {
    if end < start {
        return None;
    }
    let data = end - start;
    let total = data.checked_add(zero_fill as usize)?;
    (total <= CAPACITY).then_some(Plan { data, total })
}

/// Copies the game's template to `area` and zeroes the rest of its `total` bytes.
///
/// # Safety
/// `src` must be valid for `plan.data` bytes and `area` for `plan.total` bytes, and they must not overlap.
pub unsafe fn fill(area: *mut u8, src: *const u8, plan: Plan) {
    // SAFETY: the caller guarantees both ranges; the zero fill starts right after the copy.
    unsafe {
        core::ptr::copy_nonoverlapping(src, area, plan.data);
        core::ptr::write_bytes(area.add(plan.data), 0, plan.total - plan.data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_capacity_exactly() {
        assert_eq!(
            plan(0x1000, 0x1000 + 16, (CAPACITY - 16) as u32),
            Some(Plan {
                data: 16,
                total: CAPACITY
            })
        );
    }

    #[test]
    fn refuses_a_block_one_byte_over_the_capacity() {
        assert_eq!(plan(0x1000, 0x1000 + 16, (CAPACITY - 15) as u32), None);
    }

    #[test]
    fn refuses_an_inverted_range() {
        assert_eq!(plan(0x2000, 0x1000, 0), None);
    }

    #[test]
    fn fill_copies_the_template_then_zeroes_the_rest() {
        let template = [1u8, 2, 3, 4];
        let mut area = [0xEEu8; 8];
        let plan = Plan { data: 4, total: 8 };
        // SAFETY: both buffers cover the planned ranges and do not overlap.
        unsafe { fill(area.as_mut_ptr(), template.as_ptr(), plan) };
        assert_eq!(area, [1, 2, 3, 4, 0, 0, 0, 0]);
    }

    #[test]
    fn carrier_is_capacity_plus_64() {
        assert_eq!(CARRIER_BYTES, 262_208);
    }
}
