//! SDWA, DPP16 and DPP8 forms of the VOP1 and VOP2 encodings: port of `emu/rdna2_exec_ext.py`.
//!
//! The source 0 code of the first word (0xF9 SDWA, 0xFA DPP16, 0xE9 and 0xEA DPP8) announces a second
//! word, the extension word. The operation is the one of the plain encoding (`vector::compute`), applied
//! to operands selected, negated and permuted by the extension word, as AMD's RDNA 2 ISA describes in
//! its SDWA and DPP sections. Sources: the Python module, which follows the same sections.

use crate::error::Error;
use crate::float::{bits_float, float_bits, py_max, py_min};
use crate::vector::{Fields, compute};
use crate::wave::Wave;

const MASK32: u32 = 0xFFFF_FFFF;
pub const SDWA: u32 = 0xF9;
pub const DPP16: u32 = 0xFA;
pub const DPP8: u32 = 0xE9;
pub const DPP8_FI: u32 = 0xEA;

/// The source-code values that announce an extension word.
pub const CODES: [u32; 4] = [SDWA, DPP16, DPP8, DPP8_FI];

/// (shift, width) of the byte, word and dword selections of SDWA.
const SELECTIONS: [(u32, u32); 7] = [(0, 8), (8, 8), (16, 8), (24, 8), (0, 16), (16, 16), (0, 32)];

/// `_operand_float_flags`: whether the source and destination operands are single-precision floats.
pub(crate) fn float_flags(name: &str) -> (bool, bool) {
    if let Some(rest) = name.strip_prefix("v_cvt_") {
        let parts: Vec<&str> = rest.split('_').collect();
        let last = parts.last().copied() == Some("f32");
        let before = parts.len() >= 2 && parts[parts.len() - 2] == "f32";
        return (last, before);
    }
    if name == "v_frexp_exp_i32_f32" {
        return (true, false);
    }
    let is_f32 = name.ends_with("_f32");
    (is_f32, is_f32)
}

/// The low `width` bits, for widths up to 32.
fn width_mask(width: u32) -> u32 {
    if width >= 32 {
        MASK32
    } else {
        (1u32 << width) - 1
    }
}

/// `_extract_sel`: the selected bits of a value, sign-extended when `sext` and narrower than 32 bits.
pub(crate) fn extract_selection(value: u32, selection: u32, sext: bool) -> u32 {
    let Some(&(shift, width)) = SELECTIONS.get(selection as usize) else {
        return value;
    };
    let mask = width_mask(width);
    let mut out = (value >> shift) & mask;
    if sext && width < 32 && out & (1 << (width - 1)) != 0 {
        out |= MASK32 ^ mask;
    }
    out
}

/// `_apply_float_mods`: absolute value, then negation, on the sign bit.
pub(crate) fn float_modifiers(value: u32, absolute: bool, negated: bool) -> u32 {
    let mut out = value;
    if absolute {
        out &= 0x7FFF_FFFF;
    }
    if negated {
        out ^= 0x8000_0000;
    }
    out
}

/// `_apply_omod_clamp`: output modifier (×2, ×4, ÷2) then clamp to [0, 1].
fn output_modifier(value: u32, omod: u32, clamp: bool) -> u32 {
    if omod == 0 && !clamp {
        return value;
    }
    let mut x = bits_float(value);
    match omod {
        1 => x *= 2.0,
        2 => x *= 4.0,
        3 => x *= 0.5,
        _ => {}
    }
    if clamp {
        x = if x.is_nan() {
            0.0
        } else {
            py_min(1.0, py_max(0.0, x))
        };
    }
    float_bits(x)
}

/// `_place_dst`: the result written into the selected bits of the destination.
fn place_destination(value: u32, old: u32, selection: u32, unused: u32) -> u32 {
    if selection >= 6 {
        return value;
    }
    let (shift, width) = SELECTIONS[selection as usize];
    let field_mask = width_mask(width);
    let bits = value & field_mask;
    let mut placed = bits << shift;
    if unused == 2 {
        return (old & !(field_mask << shift)) | placed;
    }
    if unused == 1 && bits & (1 << (width - 1)) != 0 {
        let top = shift + width;
        if top < 32 {
            placed |= MASK32 << top;
        }
    }
    placed
}

/// `_dpp16_source_lane`: the lane a DPP16 control reads, and whether it is in bounds. `None` for a
/// reserved control.
fn dpp16_source_lane(lane: usize, control: u32) -> Option<(usize, bool)> {
    let row = lane & !15;
    let r = (lane & 15) as u32;
    if control <= 0xFF {
        let quad = lane & !3;
        let select = (control >> (2 * (lane & 3))) & 3;
        return Some((quad | select as usize, true));
    }
    let n = (control & 0xF) as usize;
    match control {
        0x101..=0x10F => Some(if (r as usize) + n < 16 {
            (row + r as usize + n, true)
        } else {
            (lane, false)
        }),
        0x111..=0x11F => Some(if r as usize >= n {
            (row + r as usize - n, true)
        } else {
            (lane, false)
        }),
        0x121..=0x12F => Some((row + ((r as usize + n) & 15), true)),
        0x140 => Some((row + (15 - r as usize), true)),
        0x141 => Some((row + (r as usize ^ 7), true)),
        0x150..=0x15F => Some((row + n, true)),
        0x160..=0x16F => Some((row + (r as usize ^ n), true)),
        _ => None,
    }
}

/// Executes a VOP1 or VOP2 instruction with its extension word. `fields.src0` is the extension code.
pub fn execute(wave: &mut Wave, name: &str, fields: &Fields, ext: u32) -> Result<(), Error> {
    let kind = fields.src0;
    if kind == SDWA {
        execute_sdwa(wave, name, fields, ext)
    } else if kind == DPP16 {
        execute_dpp16(wave, name, fields, ext)
    } else {
        execute_dpp8(wave, name, fields, ext, kind == DPP8_FI)
    }
}

fn operation(name: &str, a: u32, b: u32, old: u32, carry: bool) -> Result<u32, Error> {
    compute(name, a, b, old, 0, carry)
        .ok_or_else(|| Error::Untranslated(format!("instruction non traduite : {name} (SDWA/DPP)")))
}

fn execute_sdwa(wave: &mut Wave, name: &str, fields: &Fields, ext: u32) -> Result<(), Error> {
    if name == "v_sat_pk_u8_i16" {
        return Err(Error::Untranslated(format!(
            "{name} en SDWA : résultat 16 bits sous dst_sel non vérifié"
        )));
    }
    let (source_float, destination_float) = float_flags(name);
    let s0 = (
        ext & 0xFF,
        (ext >> 23) & 1 == 1,
        (ext >> 16) & 7,
        (ext >> 19) & 1 == 1,
        (ext >> 20) & 1 == 1,
        (ext >> 21) & 1 == 1,
    );
    let s1_code = fields.vsrc1.unwrap_or(0);
    let s1 = (
        s1_code,
        (ext >> 31) & 1 == 1,
        (ext >> 24) & 7,
        (ext >> 27) & 1 == 1,
        (ext >> 28) & 1 == 1,
        (ext >> 29) & 1 == 1,
    );
    let dst_selection = (ext >> 8) & 7;
    let dst_unused = (ext >> 11) & 3;
    let clamp = (ext >> 13) & 1 == 1;
    let omod = (ext >> 14) & 3;
    if !destination_float && (clamp || omod != 0) {
        return Err(Error::Untranslated(format!(
            "SDWA clamp non pris en charge sur opération entière : {name}"
        )));
    }
    let read = |wave: &Wave,
                source: (u32, bool, u32, bool, bool, bool),
                lane: usize|
     -> Result<u32, Error> {
        let (code, is_scalar, selection, sext, negated, absolute) = source;
        let raw = if is_scalar {
            wave.read_scalar(code, None, 1)? as u32
        } else {
            wave.v[code as usize][lane]
        };
        let value = extract_selection(raw, selection, if source_float { false } else { sext });
        Ok(if source_float {
            float_modifiers(value, absolute, negated)
        } else {
            value
        })
    };
    let lanes = wave.active()?;
    let carry = wave.vcc()?;
    let destination = fields.vdst as usize;
    for lane in lanes {
        let a = read(wave, s0, lane)?;
        let b = if fields.vsrc1.is_some() {
            read(wave, s1, lane)?
        } else {
            0
        };
        let old = wave.v[destination][lane];
        let mut result = operation(name, a, b, old, carry >> lane & 1 == 1)? & MASK32;
        if destination_float {
            result = output_modifier(result, omod, clamp);
        }
        wave.v[destination][lane] = place_destination(result, old, dst_selection, dst_unused);
    }
    Ok(())
}

fn execute_dpp16(wave: &mut Wave, name: &str, fields: &Fields, ext: u32) -> Result<(), Error> {
    let control = (ext >> 8) & 0x1FF;
    if dpp16_source_lane(0, control).is_none() {
        return Err(Error::Untranslated(format!(
            "dpp_ctrl réservé : 0x{control:X}"
        )));
    }
    let (source_float, _) = float_flags(name);
    let src0 = wave.v[(ext & 0xFF) as usize].clone();
    let src1 = fields.vsrc1.map(|index| wave.v[index as usize].clone());
    let fi = (ext >> 18) & 1 == 1;
    let bound_control = (ext >> 19) & 1 == 1;
    let (s0_negated, s0_absolute) = ((ext >> 20) & 1 == 1, (ext >> 21) & 1 == 1);
    let (s1_negated, s1_absolute) = ((ext >> 22) & 1 == 1, (ext >> 23) & 1 == 1);
    let bank_mask = (ext >> 24) & 0xF;
    let row_mask = (ext >> 28) & 0xF;
    let exec = wave.exec()?;
    let carry = wave.vcc()?;
    let destination = fields.vdst as usize;
    for lane in wave.active()? {
        if (row_mask >> (lane >> 4)) & 1 == 0 || (bank_mask >> ((lane & 15) >> 2)) & 1 == 0 {
            continue;
        }
        let Some((source, in_bounds)) = dpp16_source_lane(lane, control) else {
            continue;
        };
        let valid = in_bounds && (fi || (exec >> source) & 1 == 1);
        if !valid && !bound_control {
            continue;
        }
        let mut a = if valid { src0[source] } else { 0 };
        let mut b = src1.as_ref().map_or(0, |row| row[lane]);
        if source_float {
            a = float_modifiers(a, s0_absolute, s0_negated);
            b = float_modifiers(b, s1_absolute, s1_negated);
        }
        let old = wave.v[destination][lane];
        wave.v[destination][lane] = operation(name, a, b, old, carry >> lane & 1 == 1)? & MASK32;
    }
    Ok(())
}

fn execute_dpp8(
    wave: &mut Wave,
    name: &str,
    fields: &Fields,
    ext: u32,
    fi: bool,
) -> Result<(), Error> {
    let src0 = wave.v[(ext & 0xFF) as usize].clone();
    let src1 = fields.vsrc1.map(|index| wave.v[index as usize].clone());
    let selectors: Vec<usize> = (0..8)
        .map(|i| ((ext >> (8 + 3 * i)) & 7) as usize)
        .collect();
    let exec = wave.exec()?;
    let carry = wave.vcc()?;
    let destination = fields.vdst as usize;
    for lane in wave.active()? {
        let source = (lane & !7) | selectors[lane & 7];
        if !fi && (exec >> source) & 1 == 0 {
            continue;
        }
        let a = src0[source];
        let b = src1.as_ref().map_or(0, |row| row[lane]);
        let old = wave.v[destination][lane];
        wave.v[destination][lane] = operation(name, a, b, old, carry >> lane & 1 == 1)? & MASK32;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selections_pick_bytes_words_and_sign_extend() {
        assert_eq!(extract_selection(0x1234_5680, 0, false), 0x80);
        assert_eq!(extract_selection(0x1234_5680, 0, true), 0xFFFF_FF80);
        assert_eq!(extract_selection(0x1234_5680, 5, false), 0x1234);
    }

    #[test]
    fn destination_placement_keeps_or_clears_the_other_bits() {
        assert_eq!(place_destination(0xAB, 0xFFFF_FFFF, 1, 2), 0xFFFF_ABFF);
        assert_eq!(place_destination(0xAB, 0x1234_5678, 1, 0), 0xAB00);
    }

    #[test]
    fn dpp_controls_map_lanes_inside_their_row() {
        assert_eq!(dpp16_source_lane(1, 0xE4), Some((1, true))); // quad_perm identity
        assert_eq!(dpp16_source_lane(15, 0x101), Some((15, false))); // row_shl past the row end
        assert_eq!(dpp16_source_lane(0, 0x1FF), None);
    }
}
