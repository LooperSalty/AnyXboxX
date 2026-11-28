//! Half-precision arithmetic, conversions and packing in VOP1/2/3.
use anyxboxx_rdna2_ref::vop3::Fields;

use crate::{scalar::source, vector_compare::half_operand};

pub fn statement(
    name: &str,
    family: &str,
    word: u64,
    literal: Option<u32>,
    kinds: &str,
    template: &str,
) -> Result<String, String> {
    let vop3 = family == "VOP3";
    let fields = Fields::new(word);
    let (codes, d, select, abs, neg, clamp) = if vop3 {
        (
            fields.sources,
            fields.vdst,
            (word >> 11) as u32 & 15,
            fields.abs,
            fields.neg,
            fields.clamp,
        )
    } else {
        let src = word as u32 & 511;
        if matches!(src, 0xe9 | 0xea | 0xf9 | 0xfa) {
            return Err(format!("{name}: SDWA/DPP not supported"));
        }
        (
            [src, 256 + ((word >> 9) as u32 & 255), 0],
            (word >> 17) as u32 & 255,
            0,
            0,
            0,
            false,
        )
    };
    let conversion = matches!(
        name,
        "v_pack_b32_f16" | "v_min3_f16" | "v_max3_f16" | "v_med3_f16"
    ) || (name.starts_with("v_cvt_")
        && !matches!(name, "v_cvt_f16_f32" | "v_cvt_f32_f16"));
    let whole =
        name == "v_cvt_f32_f16" || name == "v_pack_b32_f16" || name.starts_with("v_cvt_pknorm_");
    let high = select & 8 != 0;
    let saturates = !conversion
        || matches!(
            name,
            "v_cvt_f16_u16" | "v_cvt_f16_i16" | "v_min3_f16" | "v_max3_f16" | "v_med3_f16"
        );
    if (vop3 && fields.omod != 0)
        || (whole && (clamp || high))
        || (clamp && !saturates)
        || (select != 0 && (name == "v_fmac_f16" || (conversion && kinds.len() == 1)))
        || (conversion && kinds.len() < 3 && select & 4 != 0)
    {
        return Err(format!("{name}: unsupported result modifier or op_sel"));
    }
    if matches!(name, "v_fmamk_f16" | "v_fmaak_f16") && literal.is_none() {
        return Err(format!("{name}: missing literal"));
    }
    let mut expr = template.to_owned();
    for (index, (kind, code)) in kinds.bytes().zip(codes).enumerate() {
        let bit = 1 << index;
        let high = select & bit != 0;
        if (high && (kind == b'w' || (128..256).contains(&code)))
            || (kind == b'i'
                && ((abs | neg) & bit != 0 || (!conversion && (240..=248).contains(&code))))
        {
            return Err(format!("{name}: unsupported source modifier or constant"));
        }
        let mut value = if code >= 256 {
            format!("v[{}]", code - 256)
        } else {
            source(code, literal).map_err(|error| format!("{name}: {error}"))?
        };
        value = match kind {
            b'w' => value,
            _ if high => format!("({value} >> 16)"),
            b'h' => half_operand(code, &value),
            _ => format!("({value} & 0xFFFFu)"),
        };
        if abs & bit != 0 {
            let mask = if kind == b'w' {
                "0x7FFFFFFFu"
            } else {
                "0x7FFFu"
            };
            value = format!("({value} & {mask})");
        }
        if neg & bit != 0 {
            let sign = if kind == b'w' {
                "0x80000000u"
            } else {
                "0x8000u"
            };
            value = format!("({value} ^ {sign})");
        }
        expr = expr.replace(["{a}", "{b}", "{c}"][index], &value);
    }
    expr = expr
        .replace("{d}", &format!("(v[{d}] & 0xFFFFu)"))
        .replace("{k}", &format!("0x{:X}u", literal.unwrap_or(0) & 0xffff));
    if clamp {
        expr = format!("F16Clamp({expr})");
    }
    if !whole {
        if conversion {
            expr = format!("({expr})");
        }
        expr = if high {
            format!("(v[{d}] & 0xFFFFu) | ({expr} << 16)")
        } else {
            format!("(v[{d}] & 0xFFFF0000u) | {expr}")
        };
    }
    let assignment = format!("v[{d}] = {expr};");
    let body = if conversion {
        format!("{{ {assignment} }}")
    } else {
        assignment
    };
    Ok(format!("if (((s[126] >> lane) & 1u) != 0) {body}"))
}
