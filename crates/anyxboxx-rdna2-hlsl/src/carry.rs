//! VOP3b arithmetic writes a VGPR result and a carry mask to its scalar destination.
use crate::{scalar::source, scalar_compare::high, wide::words};
use anyxboxx_rdna2_ref::vop3::Fields;

fn operation(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "v_add_co_u32" => ("x + y", "r < x"),
        "v_sub_co_u32" => ("x - y", "x < y"),
        "v_subrev_co_u32" => ("y - x", "y < x"),
        "v_add_co_ci_u32" => ("x + y + k", "(r < x) || (k != 0 && r == x)"),
        "v_sub_co_ci_u32" => ("x - y - k", "(x < y) || (k != 0 && x == y)"),
        "v_subrev_co_ci_u32" => ("y - x - k", "(y < x) || (k != 0 && y == x)"),
        _ => return None,
    })
}
pub fn supports(name: &str) -> bool {
    operation(name).is_some() || matches!(name, "v_mad_i64_i32" | "v_mad_u64_u32")
}
fn ballot(lanes: usize, register: u32, condition: &str) -> String {
    let target = |i| {
        if register == 125 || i == 125 {
            "s_null".into()
        } else {
            format!("s[{i}]")
        }
    };
    let lo = target(register);
    if lanes == 32 {
        format!("{lo} = Ballot(lane, {condition});")
    } else {
        format!(
            "uint2 m = Ballot64(lane, {condition}); {lo} = m.x; {} = m.y;",
            target(register + 1)
        )
    }
}
pub fn statement(
    name: &str,
    word: u64,
    literal: Option<u32>,
    lanes: usize,
) -> Result<String, String> {
    let f = Fields::new(word);
    let d = f.vdst;
    if lanes == 64 && f.sdst == 127 {
        return Err(format!("{name}: invalid scalar mask pair"));
    }
    let active = if lanes == 32 {
        "((s[126] >> lane) & 1u) != 0"
    } else {
        "LANE_ON"
    };
    let read = |code: u32| {
        if code >= 256 {
            Ok(format!("v[{}]", code - 256))
        } else {
            source(code, literal).map_err(|e| format!("{name}: {e}"))
        }
    };
    let (a, b) = (read(f.sources[0])?, read(f.sources[1])?);
    let c = f.sources[2];
    if let Some((result, flag)) = operation(name) {
        let k = if lanes == 32 {
            format!("({} >> lane) & 1u", read(c)?)
        } else if name.ends_with("_ci_u32") {
            let upper = high(c).map_err(|e| format!("{name}: {e}"))?;
            format!("(Bit64({}, {upper}, lane) ? 1u : 0u)", read(c)?)
        } else {
            "0u".into()
        };
        let written = if !f.clamp {
            "r"
        } else if name.starts_with("v_add") {
            "(o ? 0xFFFFFFFFu : r)"
        } else {
            "(o ? 0x0u : r)"
        };
        let mask = ballot(lanes, f.sdst, "o");
        return Ok(format!(
            "{{ uint x = {a}, y = {b}, k = {k}, r = 0u; bool o = false; if ({active}) {{ r = {result}; o = {flag}; v[{d}] = {written}; }} {mask} }}"
        ));
    }
    if f.clamp || (name == "v_mad_i64_i32" && f.sdst != 125) || (209..256).contains(&c) || d == 255
    {
        return Err(format!("{name}: unsupported wide multiply-add operands"));
    }
    let (lo, hi) = words(c).map_err(|e| format!("{name}: {e}"))?;
    let mul = if name == "v_mad_i64_i32" {
        "MulHiI32"
    } else {
        "MulHiU32"
    };
    let mask = ballot(lanes, f.sdst, &format!("({active}) && o"));
    Ok(format!(
        "{{ uint x = {a}, y = {b}; uint pl = x * y, ph = {mul}(x, y); uint rl = pl + {lo}; uint rh = ph + {hi}; bool o = rh < ph; uint rh2 = rh + (rl < pl ? 1u : 0u); o = o || rh2 < rh; if ({active}) {{ v[{d}] = rl; v[{}] = rh2; }} {mask} }}",
        d + 1
    ))
}
