//! FLAT, GLOBAL and SCRATCH instructions of the reference interpreter. The address of a FLAT or SCRATCH
//! access is the 64-bit VGPR pair ADDR; a GLOBAL access with SADDR (any value but 125) adds the 32-bit
//! VGPR ADDR to the 64-bit SGPR pair SADDR. The signed 12-bit OFFSET is added to the address. Port of
//! `_flat` in `emu/rdna2_exec_memory.py`; the accesses are in `vmem`.
//!
//! The 64-bit word: bits 0-11 OFFSET, 16 GLC, 32-39 ADDR, 40-47 DATA, 48-54 SADDR, 56-63 VDST.

use crate::error::Error;
use crate::vmem;
use crate::wave::Wave;

const NO_SADDR: usize = 125;
const VECTOR_REGISTERS: usize = 256;

/// The OFFSET of the word, sign-extended from 12 bits.
fn offset(word: u64) -> i64 {
    let raw = (word & 0xFFF) as i64;
    if raw & 0x800 != 0 { raw - 0x1000 } else { raw }
}

/// The address of one lane, before OFFSET is added.
fn base(wave: &Wave, family: &str, addr: usize, saddr: usize, lane: usize) -> Result<u64, Error> {
    if family == "GLOBAL" && saddr != NO_SADDR {
        let sbase = u64::from(wave.s[saddr]) | u64::from(wave.s[saddr + 1]) << 32;
        return Ok(sbase.wrapping_add(u64::from(wave.v[addr][lane])));
    }
    if addr + 1 >= VECTOR_REGISTERS {
        return Err(Error::Operand(format!(
            "registre vectoriel hors plage : v{}",
            addr + 1
        )));
    }
    Ok(u64::from(wave.v[addr][lane]) | u64::from(wave.v[addr + 1][lane]) << 32)
}

/// Executes a FLAT, GLOBAL or SCRATCH instruction whose 64-bit word is `word`, for every lane of EXEC.
pub fn execute(wave: &mut Wave, family: &str, name: &str, word: u64) -> Result<(), Error> {
    let kind = vmem::shape(name).ok_or_else(|| {
        Error::Untranslated(format!("instruction non traduite : {name} ({family})"))
    })?;
    let offset = offset(word);
    let addr = ((word >> 32) & 0xFF) as usize;
    let data = ((word >> 40) & 0xFF) as usize;
    let saddr = ((word >> 48) & 0x7F) as usize;
    let vdst = ((word >> 56) & 0xFF) as usize;
    let returns = (word >> 16) & 1 == 1;
    for lane in wave.active()? {
        let address = base(wave, family, addr, saddr, lane)?.wrapping_add_signed(offset);
        vmem::access(wave, kind, address, (data, vdst), lane, true, returns)?;
    }
    Ok(())
}

/// Whether `family` is one this module executes.
pub fn is_family(family: &str) -> bool {
    matches!(family, "FLAT" | "GLOBAL" | "SCRATCH")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_is_twelve_bits_signed() {
        assert_eq!(offset(0x7FF), 2047);
        assert_eq!(offset(0xFFF), -1);
        assert_eq!(offset(0x800), -2048);
    }
}
