//! Scalar memory instructions (SMEM) of the reference interpreter: s_load and s_buffer_load (dword to
//! dwordx16), s_store, s_atomic and s_buffer_atomic, s_memtime, and the cache-control no-ops. Port of
//! the SMEM part of `emu/rdna2_exec_memory.py` and `emu/rdna2_exec_smem.py`.
//!
//! The 64-bit word: bits 6-12 SDATA, bits 0-5 SBASE (in pairs of SGPRs), bit 16 GLC, bits 18-25 the
//! opcode. Bits 0-20 of the second word are the signed offset and bits 25-31 SOFFSET (NULL when absent,
//! and then the offset is the immediate alone). A buffer form takes its base from the V# at SBASE and
//! leaves an access outside the buffer's records: a load reads zeros, a store or an atomic does nothing.

use crate::atomic;
use crate::error::Error;
use crate::memory::{descriptor, le_words};
use crate::wave::{SCALAR_REGISTERS, Wave};

const NULL: usize = 125;
const DWORD: usize = 4;
const GLC_BIT: u32 = 16;
const CACHE_CONTROL: [&str; 5] = [
    "s_dcache_inv",
    "s_gl1_inv",
    "s_dcache_wb",
    "s_atc_probe",
    "s_atc_probe_buffer",
];
const LOAD_WORDS: [(&str, usize); 5] = [
    ("dword", 1),
    ("dwordx2", 2),
    ("dwordx4", 4),
    ("dwordx8", 8),
    ("dwordx16", 16),
];
const STORE_WORDS: [(&str, usize); 3] = [("dword", 1), ("dwordx2", 2), ("dwordx4", 4)];

/// What an SMEM mnemonic does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    MemTime,
    CacheControl,
    Load {
        buffer: bool,
        words: usize,
    },
    Store {
        buffer: bool,
        words: usize,
    },
    Atomic {
        buffer: bool,
        kind: &'static str,
        width: u32,
    },
}

/// The fields of the word, with SOFFSET already added to the offset.
struct Access {
    sdata: usize,
    sbase: usize,
    glc: bool,
    offset: i64,
}

fn words_of(table: &[(&str, usize)], suffix: &str) -> Option<usize> {
    table
        .iter()
        .find(|(name, _)| *name == suffix)
        .map(|(_, words)| *words)
}

fn shape(name: &str) -> Option<Shape> {
    if name == "s_memtime" || name == "s_memrealtime" {
        return Some(Shape::MemTime);
    }
    if CACHE_CONTROL.contains(&name) {
        return Some(Shape::CacheControl);
    }
    let (buffer, rest) = match name.strip_prefix("s_buffer_") {
        Some(rest) => (true, rest),
        None => (false, name.strip_prefix("s_")?),
    };
    if let Some(suffix) = rest.strip_prefix("load_") {
        return Some(Shape::Load {
            buffer,
            words: words_of(&LOAD_WORDS, suffix)?,
        });
    }
    if let Some(suffix) = rest.strip_prefix("store_") {
        return Some(Shape::Store {
            buffer,
            words: words_of(&STORE_WORDS, suffix)?,
        });
    }
    let suffix = rest.strip_prefix("atomic_")?;
    let (kind, wide) = match suffix.strip_suffix("_x2") {
        Some(kind) => (kind, true),
        None => (suffix, false),
    };
    let kind = atomic::KINDS.iter().find(|known| **known == kind)?;
    Some(Shape::Atomic {
        buffer,
        kind,
        width: if wide { 64 } else { 32 },
    })
}

/// Whether the interpreter executes this SMEM mnemonic.
pub fn supports(name: &str) -> bool {
    shape(name).is_some()
}

fn check(first: usize, count: usize) -> Result<(), Error> {
    if first + count > SCALAR_REGISTERS {
        return Err(Error::Operand(format!(
            "registre scalaire hors plage : s{first}"
        )));
    }
    Ok(())
}

fn signed21(value: u64) -> i64 {
    ((value as i64) << 43) >> 43
}

fn access(wave: &Wave, word: u64) -> Access {
    let soffset = ((word >> 57) & 0x7F) as usize;
    let mut offset = signed21((word >> 32) & 0x1F_FFFF);
    if soffset != NULL {
        offset += i64::from(wave.s[soffset]);
    }
    Access {
        sdata: ((word >> 6) & 0x7F) as usize,
        sbase: ((word & 0x3F) * 2) as usize,
        glc: (word >> GLC_BIT) & 1 == 1,
        offset,
    }
}

/// The address of an access of `size` bytes at `offset`, and whether it lies inside the buffer.
fn place(
    wave: &Wave,
    buffer: bool,
    sbase: usize,
    offset: i64,
    size: u64,
) -> Result<(u64, bool), Error> {
    if buffer {
        check(sbase, 3)?;
        let (base, _, records) = descriptor(wave, sbase as u32);
        let inside = offset >= 0 && offset as u64 + size <= records;
        return Ok((base.wrapping_add(offset as u64), inside));
    }
    check(sbase, 2)?;
    let base = u64::from(wave.s[sbase]) | u64::from(wave.s[sbase + 1]) << 32;
    Ok((base.wrapping_add(offset as u64), true))
}

/// The little-endian value of `count` SGPRs from `first`.
fn register_value(wave: &Wave, first: usize, count: usize) -> u64 {
    (0..count).fold(0, |value, i| {
        value | u64::from(wave.s[first + i]) << (32 * i)
    })
}

fn le_value(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .rev()
        .fold(0, |value, byte| value << 8 | u64::from(*byte))
}

fn load(wave: &mut Wave, buffer: bool, words: usize, word: u64) -> Result<(), Error> {
    let access = access(wave, word);
    check(access.sdata, words)?;
    let size = words * DWORD;
    let (address, inside) = place(wave, buffer, access.sbase, access.offset, size as u64)?;
    let data = if inside {
        wave.memory.read(address, size)
    } else {
        vec![0; size]
    };
    for (index, value) in le_words(&data).into_iter().enumerate() {
        wave.s[access.sdata + index] = value;
    }
    Ok(())
}

fn store(wave: &mut Wave, buffer: bool, words: usize, word: u64) -> Result<(), Error> {
    let access = access(wave, word);
    check(access.sdata, words)?;
    let (address, inside) = place(
        wave,
        buffer,
        access.sbase,
        access.offset,
        (words * DWORD) as u64,
    )?;
    if inside {
        let values = wave.s[access.sdata..access.sdata + words].to_vec();
        wave.memory.write_words(address, &values);
    }
    Ok(())
}

fn atomic_access(
    wave: &mut Wave,
    buffer: bool,
    kind: &str,
    width: u32,
    word: u64,
) -> Result<(), Error> {
    let access = access(wave, word);
    let count = (width / 32) as usize;
    let cmpswap = kind == "cmpswap";
    check(access.sdata, if cmpswap { 2 * count } else { count })?;
    let size = count * DWORD;
    let (address, inside) = place(wave, buffer, access.sbase, access.offset, size as u64)?;
    if !inside {
        return Ok(());
    }
    let old = le_value(&wave.memory.read(address, size));
    let data = register_value(wave, access.sdata, count);
    let compare = if cmpswap {
        register_value(wave, access.sdata + count, count)
    } else {
        0
    };
    let new = atomic::integer(kind, width, old, data, compare).ok_or_else(|| {
        Error::Untranslated(format!("instruction non traduite : atomique {kind}"))
    })?;
    wave.memory.write(address, &new.to_le_bytes()[..size]);
    if access.glc {
        for index in 0..count {
            wave.s[access.sdata + index] = (old >> (32 * index)) as u32;
        }
    }
    Ok(())
}

/// Executes an SMEM instruction whose 64-bit word is `word`. Refuses a mnemonic it does not know.
pub fn execute(wave: &mut Wave, name: &str, word: u64) -> Result<(), Error> {
    match shape(name) {
        Some(Shape::CacheControl) => Ok(()),
        Some(Shape::MemTime) => {
            let sdata = ((word >> 6) & 0x7F) as usize;
            check(sdata, 2)?;
            wave.write_scalar(sdata as u32, wave.steps, 2)
        }
        Some(Shape::Load { buffer, words }) => load(wave, buffer, words, word),
        Some(Shape::Store { buffer, words }) => store(wave, buffer, words, word),
        Some(Shape::Atomic {
            buffer,
            kind,
            width,
        }) => atomic_access(wave, buffer, kind, width, word),
        None => Err(Error::Untranslated(format!(
            "instruction non traduite : {name} (SMEM)"
        ))),
    }
}
