//! Integer atomic operations of the reference interpreter: port of `operation` in
//! `emu/rdna2_exec_atomic.py`, for the integer kinds. `integer` gives the new value from the old one
//! and the two data operands `a` and `b`, masked to the width of the access (32 or 64 bits).
//! The operands follow the Python's order: for cmpswap (vector) `a` is the written value and `b` the
//! compared one; for cmpst (DS) `a` is the compared value and `b` the written one; for wrap, `a` is
//! DATA0 and `b` DATA1; for mskor, `a` clears bits and `b` sets them.

/// The kinds the SMEM atomics accept (`smem.rs`). `integer` knows more: the DS atomics use the rest.
pub const KINDS: [&str; 13] = [
    "swap", "cmpswap", "add", "sub", "smin", "umin", "smax", "umax", "and", "or", "xor", "inc",
    "dec",
];

fn mask(width: u32) -> u64 {
    if width == 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    }
}

/// The two's-complement value of the low `width` bits of `value`.
fn signed(value: u64, width: u32) -> i64 {
    let shift = 64 - width;
    ((value << shift) as i64) >> shift
}

/// The new value of an integer atomic of `width` bits, or `None` for a kind it does not know.
pub fn integer(kind: &str, width: u32, old: u64, a: u64, b: u64) -> Option<u64> {
    let mask = mask(width);
    let (old, data, compare) = (old & mask, a & mask, b & mask);
    let value = match kind {
        "add" => old.wrapping_add(data),
        "sub" => old.wrapping_sub(data),
        "rsub" => data.wrapping_sub(old),
        "swap" | "wrxchg" => data,
        "mskor" => (old & !data) | compare,
        "wrap" => {
            if old >= data {
                old.wrapping_sub(data)
            } else {
                old.wrapping_add(compare)
            }
        }
        "csub" => old.saturating_sub(data),
        "cmpst" => {
            if old == data {
                compare
            } else {
                old
            }
        }
        "and" => old & data,
        "or" => old | data,
        "xor" => old ^ data,
        "umin" => old.min(data),
        "umax" => old.max(data),
        "smin" => signed(old, width).min(signed(data, width)) as u64,
        "smax" => signed(old, width).max(signed(data, width)) as u64,
        "inc" => {
            if old >= data {
                0
            } else {
                old + 1
            }
        }
        "dec" => {
            if old == 0 || old > data {
                data
            } else {
                old - 1
            }
        }
        "cmpswap" => {
            if old == compare {
                data
            } else {
                old
            }
        }
        _ => return None,
    };
    Some(value & mask)
}

/// The new value of an integer or float atomic (`atomic_float`), or `None` for a kind neither knows.
pub fn apply(kind: &str, width: u32, old: u64, a: u64, b: u64) -> Option<u64> {
    integer(kind, width, old, a, b).or_else(|| crate::atomic_float::float(kind, width, old, a, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inc_and_dec_wrap_at_the_data_value() {
        assert_eq!(integer("inc", 32, 5, 5, 0), Some(0));
        assert_eq!(integer("inc", 32, 4, 5, 0), Some(5));
        assert_eq!(integer("dec", 32, 0, 7, 0), Some(7));
        assert_eq!(integer("dec", 32, 9, 7, 0), Some(7));
        assert_eq!(integer("dec", 32, 3, 7, 0), Some(2));
    }

    #[test]
    fn signed_min_and_max_read_the_width() {
        assert_eq!(integer("smin", 32, 1, 0xFFFF_FFFF, 0), Some(0xFFFF_FFFF));
        assert_eq!(integer("smax", 32, 1, 0xFFFF_FFFF, 0), Some(1));
        assert_eq!(integer("umin", 32, 1, 0xFFFF_FFFF, 0), Some(1));
    }

    #[test]
    fn add_is_masked_to_the_width() {
        assert_eq!(integer("add", 32, 0xFFFF_FFFF, 2, 0), Some(1));
        assert_eq!(integer("sub", 64, 0, 1, 0), Some(u64::MAX));
    }

    #[test]
    fn cmpswap_stores_data_only_when_the_compare_matches() {
        assert_eq!(integer("cmpswap", 32, 7, 9, 7), Some(9));
        assert_eq!(integer("cmpswap", 32, 6, 9, 7), Some(6));
    }

    #[test]
    fn unknown_kinds_are_none() {
        assert_eq!(integer("fadd", 32, 0, 0, 0), None);
        assert_eq!(apply("fmul", 32, 0, 0, 0), None);
    }

    #[test]
    fn apply_takes_the_float_kinds_too() {
        assert_eq!(
            apply("fadd", 32, 0x3F80_0000, 0x3F80_0000, 0),
            Some(0x4000_0000)
        );
        assert_eq!(apply("add", 32, 1, 2, 0), Some(3));
    }

    #[test]
    fn reverse_subtract_and_exchange() {
        assert_eq!(integer("rsub", 32, 3, 10, 0), Some(7));
        assert_eq!(integer("rsub", 32, 10, 3, 0), Some(0xFFFF_FFF9));
        assert_eq!(integer("wrxchg", 64, 5, 9, 0), Some(9));
    }

    #[test]
    fn mskor_clears_then_sets() {
        assert_eq!(
            integer("mskor", 32, 0xFFFF_0000, 0xFF00_FF00, 0x0000_00AB),
            Some(0x00FF_00AB)
        );
    }

    #[test]
    fn wrap_subtracts_or_adds_data1() {
        assert_eq!(integer("wrap", 32, 10, 3, 100), Some(7));
        assert_eq!(integer("wrap", 32, 2, 3, 100), Some(102));
    }

    #[test]
    fn cmpst_compares_data0_and_writes_data1() {
        assert_eq!(integer("cmpst", 32, 7, 7, 9), Some(9));
        assert_eq!(integer("cmpst", 32, 6, 7, 9), Some(6));
    }

    #[test]
    fn compare_is_masked_to_the_width() {
        assert_eq!(integer("cmpswap", 32, 7, 9, 0x1_0000_0007), Some(9));
    }
}
