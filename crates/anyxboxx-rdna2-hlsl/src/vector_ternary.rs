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
        _ => unreachable!(),
    };
    Ok(format!(
        "if (((s[126] >> lane) & 1u) != 0) v[{}] = {expression};",
        f.vdst
    ))
}
