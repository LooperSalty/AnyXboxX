//! Resolve indexed vector moves before emitting register accesses.
pub use anyxboxx_rdna2_ref::movrel::is_vector as supports;
use anyxboxx_rdna2_ref::movrel::{check, offsets};

const ACTIVE: &str = "((s[126] >> lane) & 1u) != 0";

pub fn statement(
    name: &str,
    dest: u32,
    src: u32,
    operand: &str,
    m0: Option<u32>,
) -> Result<String, String> {
    let m0 = if name == "v_swap_b32" {
        0
    } else {
        crate::m0::known(name, m0)?
    };
    let (src_offset, dst_offset) = offsets(name, m0);
    let dest = check(name, dest, dst_offset, 255, 1)
        .map_err(|e| format!("untranslated instruction: {e}"))?;
    if name == "v_movreld_b32" {
        return Ok(format!("if ({ACTIVE}) v[{dest}] = {operand};"));
    }
    let src = src
        .checked_sub(256)
        .ok_or_else(|| format!("{name}: indexed source must be a VGPR"))?;
    let src = check(name, src, src_offset, 255, 1)
        .map_err(|e| format!("untranslated instruction: {e}"))?;
    if matches!(name, "v_swap_b32" | "v_swaprel_b32") {
        Ok(format!(
            "if ({ACTIVE}) {{ uint x = v[{src}]; v[{src}] = v[{dest}]; v[{dest}] = x; }}"
        ))
    } else {
        Ok(format!("if ({ACTIVE}) v[{dest}] = v[{src}];"))
    }
}
