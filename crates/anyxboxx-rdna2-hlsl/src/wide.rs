//! F64 and paired-word operations use the reference model's operand kinds.
use crate::{scalar::source, scalar_compare::high, wide_templates::template};
use anyxboxx_rdna2_ref::{
    float::inline_double,
    vop3::Fields,
    wide::{Kind, spec},
};

fn low(code: u32) -> Result<String, String> {
    if code >= 256 {
        Ok(format!("v[{}]", code - 256))
    } else {
        source(code, None)
    }
}
pub(crate) fn words(code: u32) -> Result<(String, String), String> {
    if let Some(bits) = inline_double(code) {
        return Ok((
            format!("0x{:X}u", bits as u32),
            format!("0x{:X}u", bits >> 32),
        ));
    }
    if code == 511 || code == 127 {
        return Err("source register pair out of bounds".into());
    }
    let hi = if code >= 256 {
        format!("v[{}]", code - 255)
    } else {
        high(code)?
    };
    Ok((low(code)?, hi))
}

pub fn statement(name: &str, family: &str, word: u64) -> Result<String, String> {
    let f = Fields::new(word);
    let (codes, d, abs, neg) = if family == "VOP3" {
        if f.clamp
            || f.omod != 0
            || (matches!(name, "v_cvt_i32_f64" | "v_cvt_u32_f64") && (f.abs | f.neg) != 0)
        {
            return Err(format!("{name}: unsupported source or output modifier"));
        }
        (f.sources, f.vdst, f.abs, f.neg)
    } else {
        ([word as u32 & 511, 0, 0], (word >> 17) as u32 & 255, 0, 0)
    };
    let (kinds, dst_kind) =
        spec(name).ok_or_else(|| format!("untranslated instruction: {name}"))?;
    if d == 255 && matches!(dst_kind, Kind::F64 | Kind::U64) {
        return Err(format!("{name}: destination register pair out of bounds"));
    }
    let mut line = template(name)
        .unwrap()
        .replace("{dest}", &d.to_string())
        .replace("{next}", &(d + 1).to_string());
    for (i, (&kind, code)) in kinds.iter().zip(codes).enumerate() {
        let key = ["a", "b", "c"][i];
        let mut value = if matches!(kind, Kind::F64 | Kind::U64) {
            let (lo, hi) = words(code).map_err(|e| format!("{name}: {e}"))?;
            line = line
                .replace(&format!("{{{key}_lo}}"), &lo)
                .replace(&format!("{{{key}_hi}}"), &hi);
            format!("asdouble({lo}, {hi})")
        } else {
            low(code).map_err(|e| format!("{name}: {e}"))?
        };
        if kind == Kind::F64 {
            if abs & (1 << i) != 0 {
                value = format!("abs({value})");
            }
            if neg & (1 << i) != 0 {
                value = format!("(-{value})");
            }
        } else if kind == Kind::F32 {
            if abs & (1 << i) != 0 {
                value = format!("({value} & 0x7FFFFFFFu)");
            }
            if neg & (1 << i) != 0 {
                value = format!("({value} ^ 0x80000000u)");
            }
        }
        line = line.replace(&format!("{{{key}}}"), &value);
    }
    Ok(line)
}
