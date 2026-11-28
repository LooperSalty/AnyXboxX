//! VOP1 wave32 moves, integer operations, and conversions.
use crate::scalar::source;
use crate::vector_movrel;

const ACTIVE: &str = "((s[126] >> lane) & 1u) != 0";

pub fn statement(
    name: &str,
    word: u32,
    literal: Option<u32>,
    m0: Option<u32>,
) -> Result<Option<String>, String> {
    let code = word & 0x1ff;
    if matches!(code, 0xe9 | 0xea | 0xf9 | 0xfa) {
        return Err(format!("untranslated instruction: {name} with SDWA or DPP"));
    }
    let a = if code >= 256 {
        format!("v[{}]", code - 256)
    } else {
        source(code, literal).map_err(|error| format!("{name}: {error}"))?
    };
    let d = (word >> 17) & 0xff;
    if vector_movrel::supports(name) {
        return vector_movrel::statement(name, d, code, &a, m0).map(Some);
    }
    if name == "v_readfirstlane_b32" {
        return Ok(Some(format!(
            "s[{d}] = ReadLane(lane, {a}, firstbitlow(s[126] == 0u ? 1u : s[126]));"
        )));
    }
    if matches!(name, "v_nop" | "v_pipeflush" | "v_clrexcp") {
        return Ok(None);
    }
    let expr = match crate::unary_constant::fold(name, code, literal, None)? {
        Some(value) => value,
        None => expression(name, &a)?,
    };
    Ok(Some(format!("if ({ACTIVE}) v[{d}] = {expr};")))
}

pub fn scaled(name: &str, value: &str, omod: u32, clamp: bool, sdwa: bool) -> Option<String> {
    if !matches!(
        name,
        "v_rcp_f32" | "v_rcp_iflag_f32" | "v_rsq_f32" | "v_sqrt_f32"
    ) {
        return None;
    }
    let scale = ["1.0", "2.0", "4.0", "0.5"][omod as usize];
    Some(format!("UnaryOutputF32({value}, {scale}, {clamp}, {sdwa})"))
}

pub fn expression(name: &str, a: &str) -> Result<String, String> {
    Ok(match name {
        "v_rcp_f32" | "v_rcp_iflag_f32" => format!("ReciprocalF32({a})"),
        "v_rsq_f32" => format!("RsqrtF32({a})"),
        "v_sqrt_f32" => format!("SqrtF32({a})"),
        "v_rndne_f32" => format!("RndneF32({a})"),
        "v_mov_b32" => a.to_owned(),
        "v_not_b32" => format!("(~{a})"),
        "v_bfrev_b32" => format!("reversebits({a})"),
        "v_ffbl_b32" => format!("firstbitlow({a})"),
        "v_ffbh_u32" => format!("({a} != 0u ? (31u - firstbithigh({a})) : 0xFFFFFFFFu)"),
        "v_ffbh_i32" => format!("LeadingSignsI32({a})"),
        "v_cvt_f32_u32" => format!("asuint((float){a})"),
        "v_cvt_f32_i32" => format!("asuint((float)asint({a}))"),
        "v_cvt_u32_f32" => format!("SatF32ToU32({a})"),
        "v_cvt_i32_f32" => format!("SatF32ToI32({a})"),
        "v_cvt_f32_ubyte0" => format!("asuint((float)({a} & 0xFFu))"),
        "v_cvt_f32_ubyte1" => format!("asuint((float)(({a} >> 8) & 0xFFu))"),
        "v_cvt_f32_ubyte2" => format!("asuint((float)(({a} >> 16) & 0xFFu))"),
        "v_cvt_f32_ubyte3" => format!("asuint((float)(({a} >> 24) & 0xFFu))"),
        "v_cvt_off_f32_i4" => format!("asuint((float)((asint({a}) << 28) >> 28) * 0.0625f)"),
        "v_cvt_flr_i32_f32" => format!("FloorToI32({a}, false)"),
        "v_cvt_rpi_i32_f32" => format!("FloorToI32({a}, true)"),
        "v_sat_pk_u8_i16" => format!(
            "((uint)clamp((int)(({a}) << 16u) >> 16, 0, 255) | ((uint)clamp((int)({a}) >> 16, 0, 255) << 8u))"
        ),
        "v_frexp_exp_i32_f32" => format!("FrexpExpF32({a})"),
        "v_frexp_mant_f32" => format!("FrexpMantF32({a})"),
        _ => return Err(format!("untranslated instruction: {name}")),
    })
}
