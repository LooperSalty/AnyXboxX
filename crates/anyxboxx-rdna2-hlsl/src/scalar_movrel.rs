//! SOP1 moves with known, checked register indices.
use crate::{scalar::source, scalar_compare::high};
pub use anyxboxx_rdna2_ref::movrel::is_scalar as supports;
use anyxboxx_rdna2_ref::{
    movrel::{check, offsets},
    scalar::fields,
};

pub fn statement(
    name: &str,
    word: u32,
    literal: Option<u32>,
    m0: Option<u32>,
) -> Result<Option<String>, String> {
    let (src_offset, dst_offset) = offsets(name, crate::m0::known(name, m0)?);
    let f = fields(word);
    let width = if name.ends_with("_b64") { 2 } else { 1 };
    let index = |reg, offset| {
        check(name, reg, offset, 105, width).map_err(|e| format!("untranslated instruction: {e}"))
    };
    let (low, upper) = if matches!(name, "s_movreld_b32" | "s_movreld_b64") {
        if width == 2 && f.ssrc0 == 255 {
            return Err(format!(
                "untranslated instruction: {name} with 64-bit literal"
            ));
        }
        let x = source(f.ssrc0, literal).map_err(|e| format!("{name}: {e}"))?;
        let y = if width == 2 && f.ssrc0 != 127 {
            high(f.ssrc0).map_err(|e| format!("{name}: {e}"))?
        } else {
            "0u".into()
        };
        (x, y)
    } else {
        let src = index(f.ssrc0, src_offset)?;
        (format!("s[{src}]"), format!("s[{}]", src + 1))
    };
    let dest = if matches!(name, "s_movrels_b32" | "s_movrels_b64") {
        if f.sdst == 125 {
            return Ok(None);
        }
        if f.sdst + width > 128 {
            return Err(format!("{name}: destination past s127"));
        }
        f.sdst
    } else {
        index(f.sdst, dst_offset)?
    };
    Ok(Some(if width == 1 {
        format!("s[{dest}] = {low};")
    } else {
        format!(
            "{{ uint x = {low}, y = {upper}; s[{dest}] = x; s[{}] = y; }}",
            dest + 1
        )
    }))
}
