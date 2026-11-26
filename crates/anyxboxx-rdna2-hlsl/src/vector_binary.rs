//! VOP2 wave32 integer arithmetic, bitwise operations and selection.
use crate::scalar::source;

pub fn statement(name: &str, word: u32, literal: Option<u32>) -> Result<String, String> {
    let code = word & 0x1ff;
    if matches!(code, 0xe9 | 0xea | 0xf9 | 0xfa) {
        return Err(format!("untranslated instruction: {name} with SDWA or DPP"));
    }
    let a = if code >= 256 {
        format!("v[{}]", code - 256)
    } else {
        source(code, literal).map_err(|e| format!("{name}: {e}"))?
    };
    let b = format!("v[{}]", (word >> 9) & 0xff);
    let d = (word >> 17) & 0xff;
    let expression = match name {
        "v_cndmask_b32" => format!("(((s[106] >> lane) & 1u) != 0 ? {b} : {a})"),
        "v_add_nc_u32" => format!("({a} + {b})"),
        "v_sub_nc_u32" => format!("({a} - {b})"),
        "v_subrev_nc_u32" => format!("({b} - {a})"),
        "v_and_b32" => format!("({a} & {b})"),
        "v_or_b32" => format!("({a} | {b})"),
        "v_xor_b32" => format!("({a} ^ {b})"),
        "v_xnor_b32" => format!("(~({a} ^ {b}))"),
        "v_lshlrev_b32" => format!("({b} << ({a} & 31u))"),
        "v_lshrrev_b32" => format!("({b} >> ({a} & 31u))"),
        "v_ashrrev_i32" => format!("asuint(asint({b}) >> ({a} & 31u))"),
        "v_min_u32" => format!("min({a}, {b})"),
        "v_max_u32" => format!("max({a}, {b})"),
        "v_min_i32" => format!("asuint(min(asint({a}), asint({b})))"),
        "v_max_i32" => format!("asuint(max(asint({a}), asint({b})))"),
        "v_mul_u32_u24" => format!("(({a} & 0xFFFFFFu) * ({b} & 0xFFFFFFu))"),
        "v_mul_hi_u32_u24" => format!("MulHiU32({a} & 0xFFFFFFu, {b} & 0xFFFFFFu)"),
        "v_mul_i32_i24" => format!("asuint(((asint({a}) << 8) >> 8) * ((asint({b}) << 8) >> 8))"),
        "v_mul_hi_i32_i24" => {
            format!("MulHiI32(asuint((asint({a}) << 8) >> 8), asuint((asint({b}) << 8) >> 8))")
        }
        _ => return Err(format!("untranslated instruction: {name}")),
    };
    Ok(format!(
        "if (((s[126] >> lane) & 1u) != 0) v[{d}] = {expression};"
    ))
}
