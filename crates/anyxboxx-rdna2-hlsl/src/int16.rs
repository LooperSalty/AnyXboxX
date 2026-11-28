//! Packed integer arithmetic and VOP3 half selection.
use crate::{int16_templates::operation, scalar::source};
use anyxboxx_rdna2_ref::vop3::Fields;

const ACTIVE: &str = "((s[126] >> lane) & 1u) != 0";
const WIDE: [&str; 6] = [
    "v_mad_u32_u16",
    "v_mad_i32_i16",
    "v_cvt_pk_u16_u32",
    "v_cvt_pk_i16_i32",
    "v_add_nc_i32",
    "v_sub_nc_i32",
];

fn operation_name(name: &str) -> &str {
    match name {
        "v_add_nc_u16" => "add_u16",
        "v_sub_nc_u16" => "sub_u16",
        "v_add_nc_i16" => "add_i16",
        "v_sub_nc_i16" => "sub_i16",
        _ => name
            .strip_prefix("v_pk_")
            .or_else(|| name.strip_prefix("v_"))
            .unwrap_or(name),
    }
}

pub fn supports(name: &str) -> bool {
    WIDE.contains(&name) || operation(operation_name(name)).is_some()
}

fn sources(name: &str, codes: &[u32], literal: Option<u32>) -> Result<String, String> {
    let mut terms = Vec::new();
    for (var, &code) in ["x", "y", "z"].into_iter().zip(codes) {
        let value = if code >= 256 {
            format!("v[{}]", code - 256)
        } else {
            source(code, literal).map_err(|err| format!("{name}: {err}"))?
        };
        terms.push(format!("{var} = {value}"));
    }
    Ok(terms.join(", "))
}

fn half(var: &str, high: bool) -> String {
    if high {
        format!("({var} >> 16)")
    } else {
        format!("({var} & 0xFFFFu)")
    }
}

fn expression(template: &str, select: u32) -> String {
    let mut result = template.to_owned();
    for (index, (key, var)) in [("{a}", "x"), ("{b}", "y"), ("{c}", "z")]
        .into_iter()
        .enumerate()
    {
        result = result.replace(key, &half(var, select & (1 << index) != 0));
    }
    result
}

pub fn statement(
    name: &str,
    packed: bool,
    word: u64,
    literal: Option<u32>,
) -> Result<String, String> {
    let f = Fields::new(word);
    let d = f.vdst;
    let select = ((word >> 11) & 15) as u32;
    if f.abs != 0 || f.neg != 0 || (!packed && f.omod != 0) {
        return Err(format!(
            "{name}: floating-point modifier on integer sources"
        ));
    }
    let op = operation_name(name);
    let Some((plain, saturated, count)) = operation(op) else {
        return wide(name, &f, select, literal);
    };
    let template = if f.clamp {
        match saturated {
            Some(value) => value,
            None if packed && (op.starts_with("max_") || op.starts_with("min_")) => plain,
            None => return Err(format!("{name}: unsupported clamp")),
        }
    } else {
        plain
    };
    let args = sources(name, &f.sources[..count], literal)?;
    let low = expression(template, select);
    if packed {
        let high_select = f.omod | ((word >> 14) as u32 & 1) << 2;
        let high = expression(template, high_select);
        return Ok(format!(
            "{{ uint {args}; uint lo = {low}, hi = {high}; if ({ACTIVE}) v[{d}] = (lo & 0xFFFFu) | (hi << 16); }}"
        ));
    }
    let store = if select & 8 != 0 {
        format!("(v[{d}] & 0xFFFFu) | (r << 16)")
    } else {
        format!("(v[{d}] & 0xFFFF0000u) | (r & 0xFFFFu)")
    };
    Ok(format!(
        "{{ uint {args}; uint r = {low}; if ({ACTIVE}) v[{d}] = {store}; }}"
    ))
}

fn wide(name: &str, f: &Fields, select: u32, literal: Option<u32>) -> Result<String, String> {
    let mad = name.starts_with("v_mad_");
    if (mad && select & 12 != 0) || (!mad && select != 0) || (name.starts_with("v_cvt_") && f.clamp)
    {
        return Err(format!("{name}: unsupported op_sel or clamp"));
    }
    let args = sources(name, &f.sources[..if mad { 3 } else { 2 }], literal)?;
    let d = f.vdst;
    let sat = f.clamp;
    if mad {
        let (a, b) = (half("x", select & 1 != 0), half("y", select & 2 != 0));
        let (product, total) = if name == "v_mad_i32_i16" {
            (
                format!("asuint(Sext16({a}) * Sext16({b}))"),
                format!("AddSatI32(p, z, {sat})"),
            )
        } else {
            (
                format!("{a} * {b}"),
                if sat {
                    "(p + z < p ? 0xFFFFFFFFu : p + z)"
                } else {
                    "p + z"
                }
                .to_owned(),
            )
        };
        return Ok(format!(
            "{{ uint {args}; uint p = {product}; uint r = {total}; if ({ACTIVE}) v[{d}] = r; }}"
        ));
    }
    let expr = match name {
        "v_cvt_pk_u16_u32" => "min(x, 0xFFFFu) | (min(y, 0xFFFFu) << 16)".into(),
        "v_cvt_pk_i16_i32" => "(SatI16(asint(x)) & 0xFFFFu) | (SatI16(asint(y)) << 16)".into(),
        "v_add_nc_i32" => format!("AddSatI32(x, y, {sat})"),
        "v_sub_nc_i32" => format!("SubSatI32(x, y, {sat})"),
        _ => return Err(format!("untranslated instruction: {name}")),
    };
    Ok(format!("{{ uint {args}; if ({ACTIVE}) v[{d}] = {expr}; }}"))
}
