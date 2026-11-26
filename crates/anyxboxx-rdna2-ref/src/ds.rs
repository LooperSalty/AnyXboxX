//! LDS instructions (DS) of the reference interpreter: ds_read and ds_write (b32 to b128, and the
//! sub-dword kinds u8, i8, u16, i16, b8, b16), the read2 and write2 forms and their st64 variant, and
//! ds_nop, the atomics of `ds_atomic` (simple and extended forms, wrxchg2), the _d16 halves of
//! `ds_half`, and the lane exchanges of `ds_lanes` (ds_swizzle, ds_permute, ds_bpermute). Port of `_lds`
//! and its helpers in `emu/rdna2_exec_memory.py` for those names. Anything else is refused with
//! `Error::Untranslated`.
//!
//! The 64-bit word: bits 0-7 OFFSET0, 8-15 OFFSET1, 32-39 ADDR, 40-47 DATA0, 48-55 DATA1, 56-63 VDST.
//! Each lane addresses the LDS of its wave (64 KiB), at ADDR + offset, modulo 64 KiB. An access that
//! would run past the end is refused (the Python slices it, which changes the array's length).

use crate::ds_atomic;
use crate::ds_half;
use crate::ds_lanes;
use crate::error::Error;
use crate::memory::le_words;
use crate::wave::Wave;

/// The size of the LDS of one wave.
pub const LDS_SIZE: usize = 64 * 1024;

/// The fields of a DS word, as the instructions read them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fields {
    pub offset0: u8,
    pub offset1: u8,
    pub addr: usize,
    pub data0: usize,
    pub data1: usize,
    pub vdst: usize,
}

impl Fields {
    pub fn new(word: u64) -> Fields {
        Fields {
            offset0: (word & 0xFF) as u8,
            offset1: ((word >> 8) & 0xFF) as u8,
            addr: ((word >> 32) & 0xFF) as usize,
            data0: ((word >> 40) & 0xFF) as usize,
            data1: ((word >> 48) & 0xFF) as usize,
            vdst: ((word >> 56) & 0xFF) as usize,
        }
    }
}
const DWORD: usize = 4;
const STRIDE64: usize = 64;
const VECTOR_REGISTERS: usize = 256;

/// What a DS read or write moves: its direction, its size in bytes, whether a sub-dword load is signed,
/// and the two-offset form (`pair`, with `st64` scaling the offsets by 64 elements).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shape {
    write: bool,
    pair: bool,
    st64: bool,
    size: usize,
    signed: bool,
}

fn kind(suffix: &str) -> Option<(usize, bool)> {
    Some(match suffix {
        "b32" => (4, false),
        "b64" => (8, false),
        "b96" => (12, false),
        "b128" => (16, false),
        "u8" | "b8" => (1, false),
        "i8" => (1, true),
        "u16" | "b16" => (2, false),
        "i16" => (2, true),
        _ => return None,
    })
}

fn shape(name: &str) -> Option<Shape> {
    let rest = name.strip_prefix("ds_")?;
    let (write, rest) = match rest.strip_prefix("read") {
        Some(rest) => (false, rest),
        None => (true, rest.strip_prefix("write")?),
    };
    let (pair, st64, rest) = if let Some(rest) = rest.strip_prefix("2st64_") {
        (true, true, rest)
    } else if let Some(rest) = rest.strip_prefix("2_") {
        (true, false, rest)
    } else {
        (false, false, rest.strip_prefix('_')?)
    };
    let (size, signed) = kind(rest)?;
    Some(Shape {
        write,
        pair,
        st64,
        size,
        signed,
    })
}

/// Whether the interpreter executes this DS mnemonic.
pub fn supports(name: &str) -> bool {
    name == "ds_nop"
        || shape(name).is_some()
        || ds_atomic::shape(name).is_some()
        || ds_half::shape(name).is_some()
        || ds_lanes::NAMES.contains(&name)
}

fn check_vgpr(first: usize, count: usize) -> Result<(), Error> {
    if first + count > VECTOR_REGISTERS {
        return Err(Error::Operand(format!(
            "registre vectoriel hors plage : v{first}"
        )));
    }
    Ok(())
}

/// The `size` bytes held by the VGPRs from `register` in `lane`, little-endian.
pub(crate) fn store_bytes(
    wave: &Wave,
    register: usize,
    lane: usize,
    size: usize,
) -> Result<Vec<u8>, Error> {
    let count = size.div_ceil(DWORD);
    check_vgpr(register, count)?;
    let mut bytes = Vec::with_capacity(count * DWORD);
    for index in 0..count {
        bytes.extend(wave.v[register + index][lane].to_le_bytes());
    }
    bytes.truncate(size);
    Ok(bytes)
}

/// Writes loaded bytes into the VGPRs from `target` in `lane`. A sub-dword load fills one VGPR, the
/// value zero-extended, or sign-extended for the signed kinds.
pub(crate) fn load_into(
    wave: &mut Wave,
    target: usize,
    lane: usize,
    data: &[u8],
    signed: bool,
) -> Result<(), Error> {
    if data.len() < DWORD {
        let bits = (data.len() * 8) as u32;
        let value = data
            .iter()
            .rev()
            .fold(0u64, |acc, byte| acc << 8 | u64::from(*byte));
        let value = if signed {
            ((value << (64 - bits)) as i64 >> (64 - bits)) as u32
        } else {
            value as u32
        };
        check_vgpr(target, 1)?;
        wave.v[target][lane] = value;
        return Ok(());
    }
    let words = le_words(data);
    check_vgpr(target, words.len())?;
    for (index, word) in words.into_iter().enumerate() {
        wave.v[target + index][lane] = word;
    }
    Ok(())
}

/// One access of a lane: `address` is the byte offset in the LDS before the modulo.
fn access(
    wave: &mut Wave,
    shape: Shape,
    lane: usize,
    address: u64,
    source: usize,
    target: usize,
) -> Result<(), Error> {
    let start = (address % LDS_SIZE as u64) as usize;
    let end = start + shape.size;
    if end > LDS_SIZE {
        return Err(Error::Operand(format!(
            "accès LDS au-delà de la fin : {start:#x}"
        )));
    }
    if shape.write {
        let bytes = store_bytes(wave, source, lane, shape.size)?;
        wave.lds[start..end].copy_from_slice(&bytes);
    } else {
        let data = wave.lds[start..end].to_vec();
        load_into(wave, target, lane, &data, shape.signed)?;
    }
    Ok(())
}

/// Executes a DS instruction whose 64-bit word is `word`, for every lane of EXEC.
pub fn execute(wave: &mut Wave, name: &str, word: u64) -> Result<(), Error> {
    if name == "ds_nop" {
        return Ok(());
    }
    if let Some(atomic) = ds_atomic::shape(name) {
        return ds_atomic::execute(wave, atomic, &Fields::new(word));
    }
    if let Some(half) = ds_half::shape(name) {
        return ds_half::execute(wave, half, &Fields::new(word));
    }
    if ds_lanes::NAMES.contains(&name) {
        return ds_lanes::execute(wave, name, &Fields::new(word));
    }
    let shape = shape(name)
        .ok_or_else(|| Error::Untranslated(format!("instruction non traduite : {name} (DS)")))?;
    let offset0 = word & 0xFF;
    let offset1 = (word >> 8) & 0xFF;
    let addr = ((word >> 32) & 0xFF) as usize;
    let data0 = ((word >> 40) & 0xFF) as usize;
    let data1 = ((word >> 48) & 0xFF) as usize;
    let vdst = ((word >> 56) & 0xFF) as usize;
    for lane in wave.active()? {
        let base = u64::from(wave.v[addr][lane]);
        let places = if shape.pair {
            let step = shape.size as u64 * if shape.st64 { STRIDE64 as u64 } else { 1 };
            [
                (base + offset0 * step, data0, vdst),
                (base + offset1 * step, data1, vdst + shape.size / DWORD),
            ]
            .to_vec()
        } else {
            vec![(base + (offset1 << 8 | offset0), data0, vdst)]
        };
        for (address, source, target) in places {
            access(wave, shape, lane, address, source, target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_give_direction_size_and_form() {
        assert_eq!(
            shape("ds_read_b32"),
            Some(Shape {
                write: false,
                pair: false,
                st64: false,
                size: 4,
                signed: false
            })
        );
        assert_eq!(
            shape("ds_write2st64_b64"),
            Some(Shape {
                write: true,
                pair: true,
                st64: true,
                size: 8,
                signed: false
            })
        );
        assert_eq!(
            shape("ds_read_i8"),
            Some(Shape {
                write: false,
                pair: false,
                st64: false,
                size: 1,
                signed: true
            })
        );
        assert_eq!(shape("ds_add_u32"), None);
        assert_eq!(shape("ds_read_b32_d16"), None);
    }

    #[test]
    fn only_the_names_it_executes_are_supported() {
        assert!(supports("ds_nop"));
        assert!(supports("ds_read2_b32"));
        assert!(supports("ds_swizzle_b32"));
        assert!(supports("ds_write_b16_d16_hi"));
        assert!(!supports("ds_write_b16_d16"));
        assert!(!supports("ds_read_b32_d16"));
    }
}
