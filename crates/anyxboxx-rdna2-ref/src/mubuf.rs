//! MUBUF instructions of the reference interpreter: buffer loads, stores and atomics addressed through
//! the V# descriptor in four SGPRs (`memory::descriptor`). Port of `_buffer` in
//! `emu/rdna2_exec_memory.py`; the accesses are in `vmem`.
//!
//! The 64-bit word: bits 0-11 OFFSET (unsigned), 12 OFFEN, 13 IDXEN, 14 GLC, 32-39 VADDR, 40-47 VDATA,
//! 48-52 SRSRC (in units of four SGPRs), 56-63 SOFFSET (NULL: 0). The address of a lane is the base of
//! the descriptor, plus SOFFSET, plus INDEX times the stride, plus the byte offset. With a stride, the
//! access is inside when its index is below the record count; without one, when the whole access fits
//! in the records. Outside, a load reads zeros and a store or an atomic does nothing.
//!
//! buffer_gl0_inv and buffer_gl1_inv invalidate caches this model does not have: they do nothing, whatever
//! their fields, as in the Python (`CACHE_CONTROL` in `emu/rdna2_exec_memory.py`).

use crate::error::Error;
use crate::memory::descriptor;
use crate::vmem;
use crate::wave::Wave;

const NULL: u32 = 125;
/// The MUBUF cache controls (`CACHE_CONTROL`).
const CACHE_CONTROL: [&str; 2] = ["buffer_gl0_inv", "buffer_gl1_inv"];
const VECTOR_REGISTERS: usize = 256;

/// The register `index` of the lane, refused past the last VGPR.
fn vgpr(wave: &Wave, index: usize, lane: usize) -> Result<u32, Error> {
    if index >= VECTOR_REGISTERS {
        return Err(Error::Operand(format!(
            "registre vectoriel hors plage : v{index}"
        )));
    }
    Ok(wave.v[index][lane])
}

/// Executes a MUBUF instruction whose 64-bit word is `word`, for every lane of EXEC.
pub fn execute(wave: &mut Wave, name: &str, word: u64) -> Result<(), Error> {
    if CACHE_CONTROL.contains(&name) {
        return Ok(());
    }
    let kind = vmem::shape(name)
        .ok_or_else(|| Error::Untranslated(format!("instruction non traduite : {name} (MUBUF)")))?;
    let size = match kind {
        vmem::Kind::Load { size, .. } | vmem::Kind::Store { size } => size as u64,
        vmem::Kind::Atomic { width, .. } => u64::from(width / 8),
    };
    let offset = word & 0xFFF;
    let offen = ((word >> 12) & 1) as usize;
    let idxen = ((word >> 13) & 1) as usize;
    let returns = (word >> 14) & 1 == 1;
    let vaddr = ((word >> 32) & 0xFF) as usize;
    let vdata = ((word >> 40) & 0xFF) as usize;
    let (base, stride, records) = descriptor(wave, ((word >> 48) & 0x1F) as u32 * 4);
    let soffset_code = ((word >> 56) & 0xFF) as u32;
    let soffset = if soffset_code == NULL {
        0
    } else {
        wave.read_scalar(soffset_code, None, 1)?
    };
    for lane in wave.active()? {
        let index = if idxen == 1 {
            u64::from(vgpr(wave, vaddr, lane)?)
        } else {
            0
        };
        let byte_offset = offset
            + if offen == 1 {
                u64::from(vgpr(wave, vaddr + idxen, lane)?)
            } else {
                0
            };
        let inside = if stride != 0 {
            index < records
        } else {
            byte_offset + size <= records
        };
        let address = base
            .wrapping_add(soffset)
            .wrapping_add(index.wrapping_mul(u64::from(stride)))
            .wrapping_add(byte_offset);
        vmem::access(wave, kind, address, (vdata, vdata), lane, inside, returns)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_controls_change_nothing_whatever_their_fields() {
        let mut wave = Wave::new(32).unwrap();
        let before = wave.clone();
        for name in CACHE_CONTROL {
            execute(&mut wave, name, u64::MAX).unwrap();
        }
        assert_eq!(wave, before);
    }

    #[test]
    fn other_names_without_a_shape_are_refused() {
        let mut wave = Wave::new(32).unwrap();
        assert!(matches!(
            execute(&mut wave, "buffer_load_format_x", 0),
            Err(Error::Untranslated(_))
        ));
    }
}
