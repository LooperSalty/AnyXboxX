//! Float atomic operations of the reference interpreter: port of the float kinds of `operation` in
//! `emu/rdna2_exec_atomic.py` (`fcmpswap`, `fcmpst`, `fadd`, `fmin`, `fmax`, on 32 or 64 bits).
//!
//! The rules are the Python model's, from the descriptions in AMD's RDNA 2 ISA guide:
//! - fcmpswap (vector) writes `a` when the old value equals `b`, fcmpst (DS) writes `b` when the old
//!   value equals `a`; float equality, so +0 = -0 and a NaN is never equal;
//! - fmin and fmax keep the other value when only one is a NaN (IEEE 754-2008 minNum and maxNum), keep
//!   the old value when both are, and on equal values (+0 and -0 included);
//! - fadd is the sum rounded to the width.
//!
//! Unverified assumption, carried from the Python: the NaN rule of fmin and fmax is taken from the
//! description of V_MIN and V_MAX, not measured on the hardware. The NaN fadd produces is the one the
//! Python's arithmetic gives on x86-64 (measured on the model): the first NaN operand, old value first,
//! quieted; the default NaN (sign set) for infinities of opposite signs. That is a property of the
//! model's host, not a documented RDNA 2 result.

const QUIET32: u64 = 0x0040_0000;
const QUIET64: u64 = 0x0008_0000_0000_0000;
const DEFAULT_NAN32: u64 = 0xFFC0_0000;
const DEFAULT_NAN64: u64 = 0xFFF8_0000_0000_0000;

/// A value of `width` bits read as a float, widened to f64 (exact for f32).
fn value(bits: u64, width: u32) -> f64 {
    if width == 32 {
        f64::from(f32::from_bits(bits as u32))
    } else {
        f64::from_bits(bits)
    }
}

/// The sum of two floats of `width` bits, as the model computes it (see the module notes for NaN).
fn add(old: u64, data: u64, width: u32) -> u64 {
    let (x, y) = (value(old, width), value(data, width));
    let (quiet, default_nan) = if width == 32 {
        (QUIET32, DEFAULT_NAN32)
    } else {
        (QUIET64, DEFAULT_NAN64)
    };
    if x.is_nan() {
        return old | quiet;
    }
    if y.is_nan() {
        return data | quiet;
    }
    let sum = x + y;
    if sum.is_nan() {
        return default_nan;
    }
    if width == 32 {
        // f32 + f32 in f64, then one rounding to f32: double rounding is harmless for an addition
        // (53 >= 2 × 24 + 2), and the overflow gives ±infinity, as the Python's `_bits`.
        u64::from((sum as f32).to_bits())
    } else {
        sum.to_bits()
    }
}

/// fmin and fmax: `data` replaces `old` when `wins(data, old)`; a lone NaN loses, two NaNs keep `old`.
fn pick(old: u64, data: u64, width: u32, wins: fn(f64, f64) -> bool) -> u64 {
    let (x, y) = (value(old, width), value(data, width));
    if x.is_nan() || y.is_nan() {
        return if x.is_nan() && !y.is_nan() { data } else { old };
    }
    if wins(y, x) { data } else { old }
}

/// The new value of a float atomic of `width` bits, or `None` for a kind that is not a float one.
pub fn float(kind: &str, width: u32, old: u64, a: u64, b: u64) -> Option<u64> {
    let mask = if width == 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    };
    let (old, a, b) = (old & mask, a & mask, b & mask);
    let equal = |p: u64, q: u64| value(p, width) == value(q, width);
    let new = match kind {
        "fcmpswap" => {
            if equal(old, b) {
                a
            } else {
                old
            }
        }
        "fcmpst" => {
            if equal(old, a) {
                b
            } else {
                old
            }
        }
        "fadd" => add(old, a, width),
        "fmin" => pick(old, a, width, |new, old| new < old),
        "fmax" => pick(old, a, width, |new, old| new > old),
        _ => return None,
    };
    Some(new & mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: u64 = 0x3F80_0000;
    const TWO: u64 = 0x4000_0000;
    const NEG_ZERO: u64 = 0x8000_0000;

    #[test]
    fn compare_swaps_treat_signed_zeros_as_equal_and_nan_as_unequal() {
        assert_eq!(float("fcmpswap", 32, NEG_ZERO, TWO, 0), Some(TWO));
        assert_eq!(
            float("fcmpswap", 32, 0x7FC0_0000, TWO, 0x7FC0_0000),
            Some(0x7FC0_0000)
        );
        assert_eq!(float("fcmpst", 32, ONE, ONE, TWO), Some(TWO));
        assert_eq!(float("fcmpst", 32, ONE, TWO, TWO), Some(ONE));
    }

    #[test]
    fn min_and_max_ignore_a_lone_nan_and_keep_old_on_ties() {
        let nan = 0x7FC0_0001;
        assert_eq!(float("fmin", 32, nan, ONE, 0), Some(ONE));
        assert_eq!(float("fmin", 32, ONE, nan, 0), Some(ONE));
        assert_eq!(float("fmax", 32, nan, 0x7FC0_0002, 0), Some(nan));
        assert_eq!(float("fmin", 32, 0, NEG_ZERO, 0), Some(0));
        assert_eq!(float("fmax", 32, ONE, TWO, 0), Some(TWO));
    }

    #[test]
    fn add_quiets_the_first_nan_and_rounds_to_the_width() {
        assert_eq!(float("fadd", 32, ONE, ONE, 0), Some(TWO));
        assert_eq!(
            float("fadd", 32, 0x7F80_0001, 0x7FC0_0002, 0),
            Some(0x7FC0_0001)
        );
        assert_eq!(float("fadd", 32, ONE, 0x7F80_0001, 0), Some(0x7FC0_0001));
        assert_eq!(
            float("fadd", 32, 0x7F80_0000, 0xFF80_0000, 0),
            Some(DEFAULT_NAN32)
        );
        assert_eq!(
            float("fadd", 32, 0x7F7F_FFFF, 0x7F7F_FFFF, 0),
            Some(0x7F80_0000)
        );
        assert_eq!(
            float("fadd", 64, 0x7FF0_0000_0000_0001, 0x3FF0_0000_0000_0000, 0),
            Some(0x7FF8_0000_0000_0001)
        );
    }

    #[test]
    fn integer_kinds_are_not_float_ones() {
        assert_eq!(float("add", 32, 0, 0, 0), None);
    }
}
