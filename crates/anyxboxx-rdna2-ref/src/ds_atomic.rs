//! LDS atomics of the reference interpreter: the simple forms (`ds_add_u32`, `ds_min_i32`...), the
//! extended forms (`ds_rsub_u64`, `ds_wrxchg_rtn_b32`, `ds_mskor_b32`, `ds_cmpst_rtn_b64`...) and the
//! two-exchange `ds_wrxchg2[st64]_rtn_b32|b64`. Port of `LDS_ATOMIC_EXT`, `LDS_EXCHANGE2`, `LDS_KINDS`
//! and `_lds_atomic`, `_lds_atomic_ext`, `_lds_exchange2` in `emu/rdna2_exec_atomic.py` and
//! `emu/rdna2_exec_memory.py`.
//!
//! Float atomics (f32, f64) are `min`, `max`, `add` and `cmpst` only, as `fmin`, `fmax`, `fadd` and
//! `fcmpst` of `atomic_float`. `min` and `max` take their kind from the type (u: unsigned, i: signed,
//! f: float) and refuse b32 and b64. `i32` exists
//! only in the simple forms. `wrxchg` and `wrap` need `_rtn`. An access past the end of the LDS is
//! refused, where the Python would slice a shorter array.

use crate::atomic;
use crate::ds::{Fields, LDS_SIZE};
use crate::error::Error;
use crate::wave::Wave;

const STRIDE64: usize = 64;
const VECTOR_REGISTERS: usize = 256;
const SIMPLE_OPS: [&str; 7] = ["add", "sub", "and", "or", "xor", "min", "max"];
const OPS: [&str; 14] = [
    "add", "sub", "rsub", "inc", "dec", "min", "max", "and", "or", "xor", "mskor", "cmpst",
    "wrxchg", "wrap",
];

/// One LDS atomic: the `atomic::apply` kind, the width in bits and whether the old value comes back.
/// `Exchange` is the two-access `wrxchg2` form, with its stride in elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Atomic {
    Op {
        kind: &'static str,
        width: u32,
        returns: bool,
    },
    Exchange {
        width: u32,
        stride: usize,
    },
}

/// The kind `atomic::apply` takes for `op` on a type whose first letter is `letter`, if it has one.
fn kind_for(op: &str, letter: char) -> Option<&'static str> {
    if letter == 'f' {
        return match op {
            "min" => Some("fmin"),
            "max" => Some("fmax"),
            "add" => Some("fadd"),
            "cmpst" => Some("fcmpst"),
            _ => None,
        };
    }
    Some(match (op, letter) {
        ("min", 'u') => "umin",
        ("max", 'u') => "umax",
        ("min", 'i') => "smin",
        ("max", 'i') => "smax",
        ("min" | "max", _) => return None,
        ("add", _) => "add",
        ("sub", _) => "sub",
        ("rsub", _) => "rsub",
        ("inc", _) => "inc",
        ("dec", _) => "dec",
        ("and", _) => "and",
        ("or", _) => "or",
        ("xor", _) => "xor",
        ("mskor", _) => "mskor",
        ("cmpst", _) => "cmpst",
        ("wrxchg", _) => "wrxchg",
        ("wrap", _) => "wrap",
        _ => return None,
    })
}

/// The width in bits and the letter of a data type (`u32`, `i64`...), if the atomics take it.
fn data_type(suffix: &str) -> Option<(u32, char)> {
    Some(match suffix {
        "u32" => (32, 'u'),
        "i32" => (32, 'i'),
        "b32" => (32, 'b'),
        "u64" => (64, 'u'),
        "i64" => (64, 'i'),
        "b64" => (64, 'b'),
        "f32" => (32, 'f'),
        "f64" => (64, 'f'),
        _ => return None,
    })
}

fn exchange(name: &str) -> Option<Atomic> {
    let rest = name.strip_prefix("ds_wrxchg2")?;
    let (stride, rest) = match rest.strip_prefix("st64_rtn_") {
        Some(rest) => (STRIDE64, rest),
        None => (1, rest.strip_prefix("_rtn_")?),
    };
    let width = match rest {
        "b32" => 32,
        "b64" => 64,
        _ => return None,
    };
    Some(Atomic::Exchange { width, stride })
}

/// What an LDS atomic mnemonic does, or `None` when it is not one (or is one the port refuses).
pub fn shape(name: &str) -> Option<Atomic> {
    if let Some(exchange) = exchange(name) {
        return Some(exchange);
    }
    let rest = name.strip_prefix("ds_")?;
    let (op, after) = OPS.iter().find_map(|op| {
        rest.strip_prefix(op)
            .and_then(|after| after.strip_prefix('_'))
            .map(|after| (*op, after))
    })?;
    let (returns, suffix) = match after.strip_prefix("rtn_") {
        Some(suffix) => (true, suffix),
        None => (false, after),
    };
    let (width, letter) = data_type(suffix)?;
    if suffix == "i32" && !SIMPLE_OPS.contains(&op) {
        return None;
    }
    if matches!(op, "wrxchg" | "wrap") && !returns {
        return None;
    }
    let kind = kind_for(op, letter)?;
    Some(Atomic::Op {
        kind,
        width,
        returns,
    })
}

fn check_vgpr(first: usize, count: usize) -> Result<(), Error> {
    if first + count > VECTOR_REGISTERS {
        return Err(Error::Operand(format!(
            "registre vectoriel hors plage : v{first}"
        )));
    }
    Ok(())
}

/// The `words` VGPRs from `register` in `lane`, as one little-endian value.
pub(crate) fn lane_value(
    wave: &Wave,
    register: usize,
    lane: usize,
    words: usize,
) -> Result<u64, Error> {
    check_vgpr(register, words)?;
    Ok((0..words).fold(0u64, |acc, index| {
        acc | u64::from(wave.v[register + index][lane]) << (32 * index)
    }))
}

/// Writes `value` over `words` VGPRs from `register` in `lane`.
pub(crate) fn set_lane_value(
    wave: &mut Wave,
    register: usize,
    lane: usize,
    value: u64,
    words: usize,
) -> Result<(), Error> {
    check_vgpr(register, words)?;
    for index in 0..words {
        wave.v[register + index][lane] = (value >> (32 * index)) as u32;
    }
    Ok(())
}

/// The start of an access of `size` bytes at `address` (taken modulo the LDS), refused past its end.
pub(crate) fn place(address: u64, size: usize) -> Result<usize, Error> {
    let start = (address % LDS_SIZE as u64) as usize;
    if start + size > LDS_SIZE {
        return Err(Error::Operand(format!(
            "accès LDS au-delà de la fin : {start:#x}"
        )));
    }
    Ok(start)
}

pub(crate) fn read_value(wave: &Wave, start: usize, size: usize) -> u64 {
    wave.lds[start..start + size]
        .iter()
        .rev()
        .fold(0u64, |acc, byte| acc << 8 | u64::from(*byte))
}

pub(crate) fn write_value(wave: &mut Wave, start: usize, size: usize, value: u64) {
    wave.lds[start..start + size].copy_from_slice(&value.to_le_bytes()[..size]);
}

fn op_lane(
    wave: &mut Wave,
    (kind, width, returns): (&str, u32, bool),
    fields: &Fields,
    lane: usize,
) -> Result<(), Error> {
    let words = width as usize / 32;
    let size = width as usize / 8;
    let address = u64::from(wave.v[fields.addr][lane])
        + (u64::from(fields.offset1) << 8 | u64::from(fields.offset0));
    let start = place(address, size)?;
    let old = read_value(wave, start, size);
    let a = lane_value(wave, fields.data0, lane, words)?;
    let b = lane_value(wave, fields.data1, lane, words)?;
    let new = atomic::apply(kind, width, old, a, b).ok_or_else(|| {
        Error::Untranslated(format!("instruction non traduite : atomique {kind}"))
    })?;
    write_value(wave, start, size, new);
    if returns {
        set_lane_value(wave, fields.vdst, lane, old, words)?;
    }
    Ok(())
}

fn exchange_lane(
    wave: &mut Wave,
    (width, stride): (u32, usize),
    fields: &Fields,
    lane: usize,
) -> Result<(), Error> {
    let words = width as usize / 32;
    let size = width as usize / 8;
    let base = u64::from(wave.v[fields.addr][lane]);
    let places = [
        (fields.offset0, fields.data0),
        (fields.offset1, fields.data1),
    ];
    for (index, (offset, source)) in places.into_iter().enumerate() {
        let address = base + u64::from(offset) * (size * stride) as u64;
        let start = place(address, size)?;
        let old = read_value(wave, start, size);
        let value = lane_value(wave, source, lane, words)?;
        write_value(wave, start, size, value);
        set_lane_value(wave, fields.vdst + index * words, lane, old, words)?;
    }
    Ok(())
}

/// Executes an LDS atomic for every lane of EXEC.
pub fn execute(wave: &mut Wave, atomic: Atomic, fields: &Fields) -> Result<(), Error> {
    for lane in wave.active()? {
        match atomic {
            Atomic::Op {
                kind,
                width,
                returns,
            } => op_lane(wave, (kind, width, returns), fields, lane)?,
            Atomic::Exchange { width, stride } => {
                exchange_lane(wave, (width, stride), fields, lane)?
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_give_kind_width_and_return() {
        assert_eq!(
            shape("ds_add_u32"),
            Some(Atomic::Op {
                kind: "add",
                width: 32,
                returns: false
            })
        );
        assert_eq!(
            shape("ds_max_rtn_i32"),
            Some(Atomic::Op {
                kind: "smax",
                width: 32,
                returns: true
            })
        );
        assert_eq!(
            shape("ds_min_rtn_u64"),
            Some(Atomic::Op {
                kind: "umin",
                width: 64,
                returns: true
            })
        );
        assert_eq!(
            shape("ds_rsub_rtn_b32"),
            Some(Atomic::Op {
                kind: "rsub",
                width: 32,
                returns: true
            })
        );
        assert_eq!(
            shape("ds_wrxchg_rtn_b32"),
            Some(Atomic::Op {
                kind: "wrxchg",
                width: 32,
                returns: true
            })
        );
    }

    #[test]
    fn min_and_max_refuse_b_types() {
        assert_eq!(shape("ds_min_b32"), None);
        assert_eq!(shape("ds_max_rtn_b64"), None);
    }

    #[test]
    fn float_types_take_min_max_add_and_cmpst_only() {
        assert_eq!(
            shape("ds_min_rtn_f32"),
            Some(Atomic::Op {
                kind: "fmin",
                width: 32,
                returns: true
            })
        );
        assert_eq!(
            shape("ds_cmpst_f64"),
            Some(Atomic::Op {
                kind: "fcmpst",
                width: 64,
                returns: false
            })
        );
        assert_eq!(shape("ds_sub_f32"), None);
        assert_eq!(shape("ds_add_src2_f32"), None);
    }

    #[test]
    fn i32_is_only_a_simple_form_and_wrxchg_or_wrap_need_rtn() {
        assert_eq!(shape("ds_rsub_i32"), None);
        assert_eq!(shape("ds_wrxchg_b32"), None);
        assert_eq!(shape("ds_wrap_u32"), None);
        assert!(shape("ds_xor_i32").is_some());
    }

    #[test]
    fn exchange_forms() {
        assert_eq!(
            shape("ds_wrxchg2_rtn_b32"),
            Some(Atomic::Exchange {
                width: 32,
                stride: 1
            })
        );
        assert_eq!(
            shape("ds_wrxchg2st64_rtn_b64"),
            Some(Atomic::Exchange {
                width: 64,
                stride: 64
            })
        );
        assert_eq!(shape("ds_wrxchg2_b32"), None);
    }
}
