//! Masked byte sums of absolute differences and the four-window forms.
use crate::{scalar::source, scalar_compare::high};
use anyxboxx_rdna2_ref::vop3::Fields;

pub fn supports(name: &str) -> bool {
    matches!(
        name,
        "v_msad_u8" | "v_qsad_pk_u16_u8" | "v_mqsad_pk_u16_u8" | "v_mqsad_u32_u8"
    )
}

pub fn statement(name: &str, word: u64, literal: Option<u32>) -> Result<String, String> {
    let f = Fields::new(word);
    if f.abs | f.neg | f.omod != 0 || f.clamp {
        return Err(format!("{name}: undocumented VOP3 modifiers"));
    }
    let [a, b, c] = f.sources;
    let d = f.vdst;
    let active = "((s[126] >> lane) & 1u) != 0";
    let low = |code| {
        if code >= 256 {
            Ok(format!("v[{}]", code - 256))
        } else {
            source(code, literal).map_err(|e| format!("{name}: {e}"))
        }
    };
    let b = low(b)?;
    if name == "v_msad_u8" {
        return Ok(format!(
            "if ({active}) v[{d}] = MsadU8({}, {b}) + {};",
            low(a)?,
            low(c)?
        ));
    }
    let upper = |code| {
        if code >= 256 {
            Ok(format!("v[{}]", code - 255))
        } else {
            high(code).map_err(|e| format!("{name}: {e}"))
        }
    };
    let quad = name == "v_mqsad_u32_u8";
    let count = if quad { 4 } else { 2 };
    if (quad && c < 256) || a == 511 || c + count > 512 || d + count > 256 {
        return Err(format!("{name}: invalid register group"));
    }
    let (lo, hi) = (low(a)?, upper(a)?);
    if quad {
        let accumulators = (0..count)
            .map(|i| format!("v[{}]", c - 256 + i))
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(format!(
            "{{ uint4 r = MqsadU32({lo}, {hi}, {b}, uint4({accumulators})); if ({active}) {{ v[{d}] = r.x; v[{}] = r.y; v[{}] = r.z; v[{}] = r.w; }} }}",
            d + 1,
            d + 2,
            d + 3
        ));
    }
    let masked = name == "v_mqsad_pk_u16_u8";
    Ok(format!(
        "{{ uint2 r = QsadPk({lo}, {hi}, {b}, uint2({}, {}), {masked}); if ({active}) {{ v[{d}] = r.x; v[{}] = r.y; }} }}",
        low(c)?,
        upper(c)?,
        d + 1
    ))
}
