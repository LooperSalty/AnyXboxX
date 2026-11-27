//! Fold host-libm operations with the reference interpreter when the operand is immediate.
use anyxboxx_rdna2_ref::{float::inline_constant, vector, vop3};

pub fn immediate(code: u32, literal: Option<u32>) -> Option<u32> {
    match code {
        125 => Some(0),
        128..=192 => Some(code - 128),
        193..=208 => Some(192u32.wrapping_sub(code)),
        255 => literal,
        _ => inline_constant(code).map(|value| value as u32),
    }
}

pub fn fold(
    name: &str,
    code: u32,
    literal: Option<u32>,
    fields: Option<&vop3::Fields>,
) -> Result<Option<String>, String> {
    if !matches!(name, "v_sin_f32" | "v_cos_f32" | "v_exp_f32" | "v_log_f32") {
        return Ok(None);
    }
    let mut bits = immediate(code, literal)
        .ok_or_else(|| format!("untranslated instruction: {name} with runtime input"))?;
    if let Some(f) = fields {
        if f.abs & 1 != 0 {
            bits &= 0x7fffffff;
        }
        if f.neg & 1 != 0 {
            bits ^= 0x80000000;
        }
    }
    let value = vector::compute(name, bits, 0, 0, 0, false)
        .ok_or_else(|| format!("untranslated instruction: {name}"))?;
    let output = fields.map_or(value, |f| vop3::output(f, value.into(), true));
    Ok(Some(format!("0x{output:X}u")))
}
