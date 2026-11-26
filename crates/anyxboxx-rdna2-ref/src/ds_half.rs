//! The _d16 forms of the DS accesses: a read fills one 16-bit half of VDST and keeps the other half, a
//! write takes the high half of DATA0 (the `_hi` forms). Port of `LDS_HALVES` and `_lds_half` in
//! `emu/rdna2_exec_memory.py`. The layout (the output keeps the unused bits, a predicate on D16) is
//! from LLVM's DSInstructions.td, as the Python notes; it is checked against the LLVM tests only in
//! the Python side.
//!
//! The reads are `ds_read_u8_d16`, `ds_read_i8_d16`, `ds_read_u16_d16`, each with an `_hi` form, and
//! the writes `ds_write_b8_d16_hi` and `ds_write_b16_d16_hi`.

use crate::ds::Fields;
use crate::ds_atomic::{place, read_value, write_value};
use crate::error::Error;
use crate::wave::Wave;

const HALF_BITS: u32 = 16;
const HALF_MASK: u32 = 0xFFFF;

/// One _d16 access: its direction, the size in bytes, whether a read sign-extends, and the high half.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Half {
    write: bool,
    size: usize,
    signed: bool,
    high: bool,
}

/// What a _d16 mnemonic does, or `None` when it is not one.
pub fn shape(name: &str) -> Option<Half> {
    let (write, rest) = match name.strip_prefix("ds_read_") {
        Some(rest) => (false, rest),
        None => (true, name.strip_prefix("ds_write_")?),
    };
    let (kind, high) = match rest.strip_suffix("_d16_hi") {
        Some(kind) => (kind, true),
        None => (rest.strip_suffix("_d16")?, false),
    };
    let (size, signed) = match (write, kind) {
        (false, "u8") => (1, false),
        (false, "i8") => (1, true),
        (false, "u16") => (2, false),
        (true, "b8") => (1, false),
        (true, "b16") => (2, false),
        _ => return None,
    };
    // A write takes its half from the high bits only, and only the _hi form exists.
    if write && !high {
        return None;
    }
    Some(Half {
        write,
        size,
        signed,
        high,
    })
}

/// The value of a read, sign- or zero-extended to 32 bits, then cut to its low 16 bits.
fn read_half(raw: u64, size: usize, signed: bool) -> u32 {
    let bits = (size * 8) as u32;
    let extended = if signed {
        ((raw << (64 - bits)) as i64 >> (64 - bits)) as u32
    } else {
        raw as u32
    };
    extended & HALF_MASK
}

fn execute_lane(wave: &mut Wave, half: Half, fields: &Fields, lane: usize) -> Result<(), Error> {
    let offset = u64::from(fields.offset1) << 8 | u64::from(fields.offset0);
    let address = u64::from(wave.v[fields.addr][lane]) + offset;
    let start = place(address, half.size)?;
    let shift = if half.high { HALF_BITS } else { 0 };
    if half.write {
        let mask = (1u32 << (8 * half.size)) - 1;
        let value = (wave.v[fields.data0][lane] >> shift) & mask;
        write_value(wave, start, half.size, u64::from(value));
        return Ok(());
    }
    let raw = read_value(wave, start, half.size);
    let loaded = read_half(raw, half.size, half.signed);
    let kept = wave.v[fields.vdst][lane] & !(HALF_MASK << shift);
    wave.v[fields.vdst][lane] = kept | loaded << shift;
    Ok(())
}

/// Executes a _d16 access for every lane of EXEC.
pub fn execute(wave: &mut Wave, half: Half, fields: &Fields) -> Result<(), Error> {
    for lane in wave.active()? {
        execute_lane(wave, half, fields, lane)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_give_direction_size_and_half() {
        assert_eq!(
            shape("ds_read_i8_d16_hi"),
            Some(Half {
                write: false,
                size: 1,
                signed: true,
                high: true
            })
        );
        assert_eq!(
            shape("ds_read_u16_d16"),
            Some(Half {
                write: false,
                size: 2,
                signed: false,
                high: false
            })
        );
        assert_eq!(
            shape("ds_write_b16_d16_hi"),
            Some(Half {
                write: true,
                size: 2,
                signed: false,
                high: true
            })
        );
        assert_eq!(shape("ds_write_b8_d16"), None);
        assert_eq!(shape("ds_read_i16_d16"), None);
    }

    #[test]
    fn a_signed_byte_is_sign_extended_then_cut_to_the_half() {
        assert_eq!(read_half(0x80, 1, true), 0xFF80);
        assert_eq!(read_half(0x80, 1, false), 0x0080);
        assert_eq!(read_half(0xFFFF, 2, false), 0xFFFF);
    }
}
