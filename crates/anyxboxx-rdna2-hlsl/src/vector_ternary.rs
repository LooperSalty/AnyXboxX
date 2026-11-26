//! Native VOP3 integer operations.
use crate::scalar::source;
use anyxboxx_rdna2_ref::vop3::Fields;

pub fn statement(name: &str, word: u64, literal: Option<u32>) -> Result<String, String> {
    let f = Fields::new(word);
    if !matches!(
        name,
        "v_add3_u32"
            | "v_lshl_add_u32"
            | "v_add_lshl_u32"
            | "v_lshl_or_b32"
            | "v_and_or_b32"
            | "v_or3_b32"
            | "v_bfe_u32"
            | "v_bfe_i32"
            | "v_bfi_b32"
            | "v_bfm_b32"
            | "v_alignbit_b32"
            | "v_alignbyte_b32"
            | "v_perm_b32"
            | "v_min3_u32"
            | "v_max3_u32"
            | "v_med3_u32"
            | "v_min3_i32"
            | "v_max3_i32"
            | "v_med3_i32"
            | "v_mul_lo_u32"
            | "v_mul_hi_u32"
            | "v_mul_hi_i32"
            | "v_mad_u32_u24"
            | "v_mad_i32_i24"
            | "v_bcnt_u32_b32"
    ) || f.clamp
    {
        return Err(format!("untranslated instruction: {name}"));
    }
    let operand = |code| {
        if code >= 256 {
            Ok(format!("v[{}]", code - 256))
        } else {
            source(code, literal).map_err(|e| format!("{name}: {e}"))
        }
    };
    let [a, b, c] = f.sources.map(operand);
    let (a, b, c) = (a?, b?, c?);
    let expression = match name {
        "v_add3_u32" => format!("({a} + {b} + {c})"),
        "v_lshl_add_u32" => format!("(({a} << ({b} & 31u)) + {c})"),
        "v_add_lshl_u32" => format!("(({a} + {b}) << ({c} & 31u))"),
        "v_lshl_or_b32" => format!("(({a} << ({b} & 31u)) | {c})"),
        "v_and_or_b32" => format!("(({a} & {b}) | {c})"),
        "v_or3_b32" => format!("({a} | {b} | {c})"),
        "v_bfe_u32" => format!("(({a} >> ({b} & 31u)) & ((1u << ({c} & 31u)) - 1u))"),
        "v_bfe_i32" => format!("BfeI32({a}, {b}, {c})"),
        "v_bfi_b32" => format!("(({a} & {b}) | (~{a} & {c}))"),
        "v_bfm_b32" => format!("(((1u << ({a} & 31u)) - 1u) << ({b} & 31u))"),
        "v_alignbit_b32" => format!("AlignBit({a}, {b}, {c})"),
        "v_alignbyte_b32" => format!("AlignBit({a}, {b}, ({c} & 3u) * 8u)"),
        "v_perm_b32" => format!("PermB32({a}, {b}, {c})"),
        "v_min3_u32" => format!("min(min({a}, {b}), {c})"),
        "v_max3_u32" => format!("max(max({a}, {b}), {c})"),
        "v_med3_u32" => format!("max(min({a}, {b}), min(max({a}, {b}), {c}))"),
        "v_min3_i32" => format!("asuint(min(min(asint({a}), asint({b})), asint({c})))"),
        "v_max3_i32" => format!("asuint(max(max(asint({a}), asint({b})), asint({c})))"),
        "v_med3_i32" => format!("asuint(max(min(asint({a}), asint({b})), min(max(asint({a}), asint({b})), asint({c}))))"),
        "v_mul_lo_u32" => format!("({a} * {b})"),
        "v_mul_hi_u32" => format!("MulHiU32({a}, {b})"),
        "v_mul_hi_i32" => format!("MulHiI32({a}, {b})"),
        "v_mad_u32_u24" => format!("(({a} & 0xFFFFFFu) * ({b} & 0xFFFFFFu) + {c})"),
        "v_mad_i32_i24" => format!("asuint(((asint({a}) << 8) >> 8) * ((asint({b}) << 8) >> 8) + asint({c}))"),
        "v_bcnt_u32_b32" => format!("(countbits({a}) + {b})"),
        _ => unreachable!(),
    };
    Ok(format!(
        "if (((s[126] >> lane) & 1u) != 0) v[{}] = {expression};",
        f.vdst
    ))
}
