//! The four division instructions supported by the Python HLSL model.
use crate::{scalar::source, wide::words};
use anyxboxx_rdna2_ref::vop3::Fields;

const ACTIVE: &str = "((s[126] >> lane) & 1u) != 0";
pub fn supports(name: &str) -> bool {
    matches!(
        name,
        "v_div_fixup_f32" | "v_div_fixup_f64" | "v_div_fmas_f32" | "v_div_scale_f32"
    )
}
fn modified(mut expr: String, abs: bool, neg: bool) -> String {
    if abs {
        expr = format!("({expr} & 0x7FFFFFFFu)");
    }
    if neg {
        expr = format!("({expr} ^ 0x80000000u)");
    }
    expr
}
pub fn statement(name: &str, word: u64, literal: Option<u32>) -> Result<String, String> {
    let f = Fields::new(word);
    if f.clamp || f.omod != 0 {
        return Err(format!("{name}: unsupported clamp or omod"));
    }
    let d = f.vdst;
    let mut args = Vec::new();
    for (index, code) in f.sources.into_iter().enumerate() {
        let abs = name != "v_div_scale_f32" && (f.abs >> index) & 1 != 0;
        let neg = (f.neg >> index) & 1 != 0;
        args.push(if name == "v_div_fixup_f64" {
            let (low, high) = words(code).map_err(|e| format!("{name}: {e}"))?;
            format!("uint2({low}, {})", modified(high, abs, neg))
        } else {
            let value = if code >= 256 {
                format!("v[{}]", code - 256)
            } else {
                source(code, literal).map_err(|e| format!("{name}: {e}"))?
            };
            modified(value, abs, neg)
        });
    }
    let args = args.join(", ");
    Ok(match name {
        "v_div_scale_f32" => {
            let target = if f.sdst == 125 {
                "s_null".into()
            } else {
                format!("s[{}]", f.sdst)
            };
            format!(
                "{{ bool o = false; if ({ACTIVE}) v[{d}] = DivScaleF32({args}, o); {target} = Ballot(lane, ({ACTIVE}) && o); }}"
            )
        }
        "v_div_fmas_f32" => {
            format!("if ({ACTIVE}) v[{d}] = DivFmasF32({args}, ((s[106] >> lane) & 1u) != 0u);")
        }
        "v_div_fixup_f32" => format!("if ({ACTIVE}) v[{d}] = DivFixupF32({args});"),
        _ if d < 255 => format!(
            "if ({ACTIVE}) {{ uint2 r = DivFixupF64({args}); v[{d}] = r.x; v[{}] = r.y; }}",
            d + 1
        ),
        _ => return Err(format!("{name}: destination register pair out of bounds")),
    })
}
