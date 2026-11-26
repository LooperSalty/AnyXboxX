//! Lane exchanges through the DS unit, without LDS access: ds_swizzle_b32, ds_permute_b32 and
//! ds_bpermute_b32. Port of `emu/rdna2_exec_lanes.py`, with the offset constants of
//! `emu/rdna2_swizzle.py`.
//!
//! The lanes are exchanged within groups of 32 (a wave64 is two halves that exchange alone). A source
//! lane that is not active gives 0. Only active lanes write VDST. ds_swizzle's ROTATE and FFT modes
//! (offset from 0xC000) and its reserved offsets are refused: the rotate and FFT pseudo-code does not
//! say how the index stays in its group.

use crate::ds::Fields;
use crate::error::Error;
use crate::wave::Wave;

const GROUP: usize = 32;
const DWORD: u64 = 4;
const QUAD_PERM_MASK: u64 = 0xFF00;
const QUAD_PERM: u64 = 0x8000;
const BITMASK_PERM_MASK: u64 = 0x8000;
const ROTATE_MODE: u64 = 0xC000;
const FIVE_BITS: u64 = 0x1F;

/// The mnemonics of this module.
pub const NAMES: [&str; 3] = ["ds_swizzle_b32", "ds_permute_b32", "ds_bpermute_b32"];

/// The first lane of the group of 32 that holds `lane`.
fn group_start(lane: usize) -> usize {
    lane & !(GROUP - 1)
}

/// The lane that `lane` reads for a ds_swizzle_b32 offset.
fn swizzle_source(offset: u64, lane: usize) -> Result<usize, Error> {
    if offset >= ROTATE_MODE {
        return Err(Error::Untranslated(format!(
            "ds_swizzle_b32 : mode ROTATE/FFT (offset 0x{offset:04X}) non vérifié"
        )));
    }
    if offset & QUAD_PERM_MASK == QUAD_PERM {
        let quad = lane % 4;
        let pick = (offset >> (2 * quad)) & 0x3;
        return Ok(lane - quad + pick as usize);
    }
    if offset & BITMASK_PERM_MASK == 0 {
        let and = offset & FIVE_BITS;
        let or = (offset >> 5) & FIVE_BITS;
        let xor = (offset >> 10) & FIVE_BITS;
        let lane = lane as u64;
        return Ok((((lane & and) | or) ^ xor) as usize);
    }
    Err(Error::Untranslated(format!(
        "ds_swizzle_b32 : offset réservé 0x{offset:04X}"
    )))
}

/// The lane a bpermute or permute of `lane` reaches: ((ADDR + offset) / 4) modulo 32 in its group.
fn target(wave: &Wave, addr: usize, lane: usize, offset: u64) -> usize {
    let index = (u64::from(wave.v[addr][lane]) + offset) / DWORD % GROUP as u64;
    group_start(lane) | index as usize
}

/// The DATA0 value of every lane, 0 for the lanes that are not active.
fn sources(wave: &Wave, data0: usize, active: &[bool]) -> Vec<u32> {
    (0..wave.lanes)
        .map(|lane| if active[lane] { wave.v[data0][lane] } else { 0 })
        .collect()
}

/// Executes one of `NAMES` for the active lanes of the wave.
pub fn execute(wave: &mut Wave, name: &str, fields: &Fields) -> Result<(), Error> {
    let offset = u64::from(fields.offset1) << 8 | u64::from(fields.offset0);
    let active_lanes = wave.active()?;
    let mut active = vec![false; wave.lanes];
    for lane in &active_lanes {
        active[*lane] = true;
    }
    let source = sources(wave, fields.data0, &active);
    let mut results = Vec::with_capacity(active_lanes.len());
    match name {
        "ds_swizzle_b32" => {
            for lane in &active_lanes {
                let picked = group_start(*lane) | swizzle_source(offset, lane % GROUP)?;
                results.push((*lane, source[picked]));
            }
        }
        "ds_bpermute_b32" => {
            for lane in &active_lanes {
                let picked = target(wave, fields.addr, *lane, offset);
                results.push((*lane, source[picked]));
            }
        }
        _ => {
            // ds_permute_b32: the active lanes in ascending order, so the greater lane wins.
            let mut forwarded = vec![0u32; wave.lanes];
            for lane in &active_lanes {
                forwarded[target(wave, fields.addr, *lane, offset)] = source[*lane];
            }
            for lane in &active_lanes {
                results.push((*lane, forwarded[*lane]));
            }
        }
    }
    for (lane, value) in results {
        wave.v[fields.vdst][lane] = value;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_perm_picks_a_lane_of_its_quad() {
        // 0xB1: lane 0 reads 1, lane 1 reads 0, lane 2 reads 3, lane 3 reads 2 (inside the quad).
        assert_eq!(swizzle_source(0x80B1, 0).unwrap(), 1);
        assert_eq!(swizzle_source(0x80B1, 5).unwrap(), 4);
        assert_eq!(swizzle_source(0x80B1, 7).unwrap(), 6);
    }

    #[test]
    fn bitmask_reverse_and_swap() {
        // and = 0x1F, xor = 0x1F: lane i reads 31 - i.
        assert_eq!(swizzle_source(0x7C1F, 0).unwrap(), 31);
        assert_eq!(swizzle_source(0x7C1F, 6).unwrap(), 25);
        // and = 0x1F, xor = 1: lanes swap in pairs.
        assert_eq!(swizzle_source(0x041F, 4).unwrap(), 5);
    }

    #[test]
    fn rotate_and_reserved_offsets_are_refused() {
        assert!(matches!(
            swizzle_source(0xC000, 0),
            Err(Error::Untranslated(_))
        ));
        assert!(matches!(
            swizzle_source(0x8100, 0),
            Err(Error::Untranslated(_))
        ));
    }

    #[test]
    fn groups_of_32_keep_their_half() {
        assert_eq!(group_start(0), 0);
        assert_eq!(group_start(31), 0);
        assert_eq!(group_start(40), 32);
    }
}
