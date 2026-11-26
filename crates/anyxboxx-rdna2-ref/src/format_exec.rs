//! Typed buffer accesses of the reference interpreter: buffer_load/store_format_* (MUBUF, format of the
//! V#) and tbuffer_load/store_format_* (MTBUF, format of the instruction). Port of `execute` and
//! `_results` in `emu/rdna2_exec_format.py`, including packed d16 forms.
//!
//! The address and the bounds are those of the other MUBUF accesses: base + SOFFSET + index x stride +
//! byte offset; with a stride the element is inside when its index is below NUM_RECORDS, otherwise
//! when offset + element size <= NUM_RECORDS. Outside, a load reads zero components (DST_SEL = 1 stays
//! 1) and a store writes nothing. An address not aligned on the format inside the buffer is refused.

use crate::buffer_format::Format;
use crate::error::Error;
use crate::format_access::{self, Access, SELECT_ONE, SELECT_X, SELECT_ZERO};
use crate::format_convert::{decode, encode, select_one};
use crate::format_d16;
use crate::memory::descriptor;
use crate::wave::{NULL, Wave};

pub use crate::format_access::supports;

const VECTOR_REGISTERS: usize = 256;

fn vgpr(wave: &Wave, index: usize, lane: usize) -> Result<u32, Error> {
    if index >= VECTOR_REGISTERS {
        return Err(Error::Operand(format!(
            "registre vectoriel hors plage : v{index}"
        )));
    }
    Ok(wave.v[index][lane])
}

fn set_vgpr(wave: &mut Wave, index: usize, lane: usize, value: u32) -> Result<(), Error> {
    if index >= VECTOR_REGISTERS {
        return Err(Error::Operand(format!(
            "registre vectoriel hors plage : v{index}"
        )));
    }
    wave.v[index][lane] = value;
    Ok(())
}

/// The values a load writes to its VGPRs: the components in order (MTBUF) or chosen by DST_SEL (MUBUF).
fn results(format: &Format, access: &Access, word3: u32, components: &[u32]) -> Vec<u32> {
    if access.typed {
        return components[..access.count].to_vec();
    }
    format_access::selects(word3)[..access.count]
        .iter()
        .map(|&select| match select {
            SELECT_ZERO => 0,
            SELECT_ONE => select_one(format),
            _ => components[(select - SELECT_X) as usize],
        })
        .collect()
}

/// The element address of one lane and whether it is inside the buffer.
fn lane_address(
    wave: &Wave,
    access: &Access,
    (base, stride, records): (u64, u32, u64),
    soffset: u64,
    lane: usize,
    size: usize,
) -> Result<(u64, bool), Error> {
    let index = if access.idxen {
        u64::from(vgpr(wave, access.vaddr, lane)?)
    } else {
        0
    };
    let byte_offset = access.offset
        + if access.offen {
            u64::from(vgpr(wave, access.vaddr + usize::from(access.idxen), lane)?)
        } else {
            0
        };
    let inside = if stride != 0 {
        index < records
    } else {
        byte_offset + size as u64 <= records
    };
    let address = base
        .wrapping_add(soffset)
        .wrapping_add(index.wrapping_mul(u64::from(stride)))
        .wrapping_add(byte_offset);
    Ok((address, inside))
}

/// Executes a typed access whose 64-bit word is `word`, for every lane of EXEC.
pub fn execute(wave: &mut Wave, name: &str, word: u64) -> Result<(), Error> {
    let access = format_access::access(name, word)?;
    let words: [u32; 4] = std::array::from_fn(|i| wave.s[access.srsrc + i]);
    let (format, enabled) = format_access::resolve(name, &access, &words)?;
    let buffer = descriptor(wave, access.srsrc as u32);
    let soffset = if access.soffset == NULL {
        0
    } else {
        wave.read_scalar(access.soffset, None, 1)?
    };
    for lane in wave.active()? {
        let (address, inside) = lane_address(wave, &access, buffer, soffset, lane, format.size)?;
        let inside = enabled && inside;
        if inside && address % format.align != 0 {
            return Err(Error::Untranslated(format!(
                "{name} : adresse 0x{address:X} non alignée pour le format {}",
                format.name()
            )));
        }
        if access.store {
            if inside {
                let values = (0..format.count())
                    .map(|i| {
                        if access.d16 {
                            let shift = if access.high { 16 } else { (i % 2) * 16 };
                            let reg = if access.high { 0 } else { i / 2 };
                            let half = (vgpr(wave, access.vdata + reg, lane)? >> shift) & 0xffff;
                            Ok(format_d16::widen(format, half, format.widths[i]))
                        } else {
                            vgpr(wave, access.vdata + i, lane)
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                wave.memory.write(address, &encode(format, &values));
            }
            continue;
        }
        let components = if inside {
            let data = wave.memory.read(address, format.size);
            if access.d16 {
                format_d16::decode(format, &data)
            } else {
                decode(format, &data)
            }
        } else {
            vec![0; format.count()]
        };
        for (i, value) in results(format, &access, words[3], &components)
            .into_iter()
            .enumerate()
        {
            if access.d16 {
                let shift = if access.high { 16 } else { (i % 2) * 16 };
                let reg = access.vdata + if access.high { 0 } else { i / 2 };
                let old = vgpr(wave, reg, lane)?;
                let half = format_d16::narrow(format, value);
                set_vgpr(wave, reg, lane, old & !(0xffff << shift) | half << shift)?;
            } else {
                set_vgpr(wave, access.vdata + i, lane, value)?;
            }
        }
    }
    Ok(())
}
