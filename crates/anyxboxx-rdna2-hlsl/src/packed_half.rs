//! Packed binary16 arithmetic, mixed FMA and the packed dot product.
use crate::scalar::source;
use anyxboxx_rdna2_ref::{half::inline_half, vop3::Fields};

const VARS: [&str; 3] = ["x", "y", "z"];

fn function(name: &str) -> Option<&'static str> {
    Some(match name {
        "v_pk_add_f16" => "F16Add",
        "v_pk_mul_f16" => "F16Mul",
        "v_pk_min_f16" => "F16Min",
        "v_pk_max_f16" => "F16Max",
        "v_pk_fma_f16" | "v_pk_fmac_f16" => "F16Fma",
        _ => return None,
    })
}
pub fn supports(name: &str) -> bool {
    function(name).is_some()
        || matches!(
            name,
            "v_fma_mix_f32" | "v_fma_mixlo_f16" | "v_fma_mixhi_f16" | "v_dot2_f32_f16"
        )
}

fn half(code: u32, var: &str, high: bool, neg: bool) -> String {
    if let Some(value) = inline_half(code) {
        return format!("0x{:X}u", value ^ if neg { 0x8000 } else { 0 });
    }
    let expr = if high {
        format!("({var} >> 16)")
    } else {
        format!("({var} & 0xFFFFu)")
    };
    if neg {
        format!("({expr} ^ 0x8000u)")
    } else {
        expr
    }
}
fn clamp(expr: String, function: &str, enabled: bool) -> String {
    if enabled {
        format!("{function}({expr})")
    } else {
        expr
    }
}

pub fn statement(
    name: &str,
    family: &str,
    word: u64,
    literal: Option<u32>,
) -> Result<String, String> {
    let f = Fields::new(word);
    let fmac = name == "v_pk_fmac_f16";
    let vop2 = family == "VOP2";
    let (d, codes) = if vop2 {
        (
            (word >> 17) as u32 & 255,
            [word as u32 & 511, 256 + ((word >> 9) as u32 & 255), 0],
        )
    } else {
        (f.vdst, f.sources)
    };
    let saturate = !vop2 && f.clamp;
    let mixed = name.starts_with("v_fma_mix");
    let dot = name == "v_dot2_f32_f16";
    let count = if mixed || dot || name == "v_pk_fma_f16" {
        3
    } else {
        2
    };
    let sel = (word >> 11) as u32 & 7;
    let hi = f.omod | ((word >> 14) as u32 & 1) << 2;
    if fmac
        && ((vop2 && matches!(codes[0], 0xe9 | 0xea | 0xf9 | 0xfa))
            || (!vop2 && (f.abs | f.neg | f.omod | ((word >> 11) as u32 & 15)) != 0))
        || mixed && sel & !hi != 0
        || dot && (sel | f.abs) & 4 != 0
    {
        return Err(format!("{name}: unsupported packed source modifier"));
    }
    let mut bindings = Vec::new();
    for i in 0..count {
        let code = codes[i];
        let uses_half = if mixed {
            hi & (1 << i) != 0
        } else {
            !(dot && i == 2)
        };
        if uses_half && inline_half(code).is_some() {
            continue;
        }
        let value = if code >= 256 {
            format!("v[{}]", code - 256)
        } else {
            source(code, literal).map_err(|e| format!("{name}: {e}"))?
        };
        bindings.push(format!("{} = {value}", VARS[i]));
    }
    let prefix = if bindings.is_empty() {
        String::new()
    } else {
        format!("uint {}; ", bindings.join(", "))
    };
    let expr = if let Some(function) = function(name) {
        let halves = [false, true].map(|upper| {
            let mut args = Vec::new();
            for i in 0..count {
                let bit = 1 << i;
                let high = if fmac {
                    upper
                } else {
                    (if upper { hi } else { sel }) & bit != 0
                };
                let neg = !fmac && (if upper { f.abs } else { f.neg }) & bit != 0;
                args.push(half(codes[i], VARS[i], high, neg));
            }
            if fmac {
                args.push(if upper {
                    format!("v[{d}] >> 16")
                } else {
                    format!("v[{d}] & 0xFFFFu")
                });
            }
            clamp(
                format!("{function}({})", args.join(", ")),
                "F16Clamp",
                saturate,
            )
        });
        format!("({}) | (({}) << 16)", halves[0], halves[1])
    } else if mixed {
        let args: Vec<_> = (0..3)
            .map(|i| {
                let bit = 1 << i;
                let mut expr = if hi & bit != 0 {
                    format!(
                        "F16ToF32({})",
                        half(codes[i], VARS[i], sel & bit != 0, false)
                    )
                } else {
                    VARS[i].to_owned()
                };
                if f.abs & bit != 0 {
                    expr = format!("({expr} & 0x7FFFFFFFu)");
                }
                if f.neg & bit != 0 {
                    expr = format!("({expr} ^ 0x80000000u)");
                }
                expr
            })
            .collect();
        let sum = format!("FmaF32({})", args.join(", "));
        if name == "v_fma_mix_f32" {
            clamp(sum, "F32Clamp01", saturate)
        } else {
            let h = clamp(format!("F32ToF16({sum})"), "F16Clamp", saturate);
            if name == "v_fma_mixhi_f16" {
                format!("(v[{d}] & 0xFFFFu) | (({h}) << 16)")
            } else {
                format!("(v[{d}] & 0xFFFF0000u) | ({h})")
            }
        }
    } else {
        let product = |select, neg| {
            let args: [String; 2] = [0usize, 1].map(|i| {
                format!(
                    "F16ToF32({})",
                    half(
                        codes[i],
                        VARS[i],
                        select & (1u32 << i) != 0u32,
                        neg & (1u32 << i) != 0u32
                    )
                )
            });
            format!("F32Mul({}, {})", args[0], args[1])
        };
        let z = if f.neg & 4 != 0 {
            "(z ^ 0x80000000u)"
        } else {
            "z"
        };
        clamp(
            format!(
                "F32Add(F32Add({}, {}), {z})",
                product(sel, f.neg),
                product(hi, f.abs)
            ),
            "F32Clamp01",
            saturate,
        )
    };
    Ok(format!(
        "{{ {prefix}if (((s[126] >> lane) & 1u) != 0) v[{d}] = {expr}; }}"
    ))
}
