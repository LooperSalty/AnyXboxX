//! Sub-dword addressing and lane permutations on VOP1, VOP2 and VOPC.
use crate::{scalar::source, vector_binary, vector_compare, vector_unary};

const ACTIVE: &str = "((s[126] >> lane) & 1u) != 0";
fn selection(sel: u32) -> (u32, u32) {
    match sel {
        0..=3 => (8 * sel, 8),
        4 | 5 => (16 * (sel - 4), 16),
        _ => (0, 32),
    }
}
fn extract(value: String, sel: u32, signed: bool) -> String {
    let (shift, width) = selection(sel);
    if width == 32 {
        return value;
    }
    let mask = (1u32 << width) - 1;
    let expr = if shift == 0 {
        format!("({value} & 0x{mask:X}u)")
    } else {
        format!("(({value} >> {shift}u) & 0x{mask:X}u)")
    };
    if signed {
        format!(
            "((({expr} & 0x{:X}u) != 0u) ? ({expr} | 0x{:X}u) : {expr})",
            1u32 << (width - 1),
            !mask
        )
    } else {
        expr
    }
}
fn place(expr: String, d: u32, sel: u32, unused: u32) -> String {
    let (shift, width) = selection(sel);
    if width == 32 {
        return expr;
    }
    let mask = (1u32 << width) - 1;
    let bits = format!("({expr} & 0x{mask:X}u)");
    let placed = if shift == 0 {
        bits.clone()
    } else {
        format!("({bits} << {shift}u)")
    };
    match unused {
        2 => format!("((v[{d}] & 0x{:X}u) | {placed})", !(mask << shift)),
        1 if shift + width < 32 => format!(
            "((({bits} & 0x{:X}u) != 0u) ? ({placed} | 0x{:X}u) : {placed})",
            1u32 << (width - 1),
            u32::MAX << (shift + width)
        ),
        _ => placed,
    }
}
fn float_mods(mut expr: String, abs: bool, neg: bool) -> String {
    if abs {
        expr = format!("({expr} & 0x7FFFFFFFu)");
    }
    if neg {
        expr = format!("({expr} ^ 0x80000000u)");
    }
    expr
}
fn row(ctrl: u32) -> Result<(String, String), String> {
    let i = ctrl & 15;
    let (source, valid) = match ctrl {
        0..=255 => (
            format!("((lane & ~3u) | ((0x{ctrl:X}u >> (2u * (lane & 3u))) & 3u))"),
            "true".into(),
        ),
        0x101..=0x10f => (
            format!("((lane & ~15u) + (lane & 15u) + {i}u)"),
            format!("((lane & 15u) + {i}u < 16u)"),
        ),
        0x111..=0x11f => (
            format!("((lane & ~15u) + (lane & 15u) - {i}u)"),
            format!("((lane & 15u) >= {i}u)"),
        ),
        0x121..=0x12f => (
            format!("((lane & ~15u) + (((lane & 15u) + {i}u) & 15u))"),
            "true".into(),
        ),
        0x140 => (
            "((lane & ~15u) + (15u - (lane & 15u)))".into(),
            "true".into(),
        ),
        0x141 => (
            "((lane & ~15u) + ((lane & 15u) ^ 7u))".into(),
            "true".into(),
        ),
        0x150..=0x15f => (format!("((lane & ~15u) + {i}u)"), "true".into()),
        0x160..=0x16f => (
            format!("((lane & ~15u) + ((lane & 15u) ^ {i}u))"),
            "true".into(),
        ),
        _ => return Err(format!("reserved dpp_ctrl 0x{ctrl:X}")),
    };
    Ok((source, valid))
}
pub fn statement(name: &str, family: &str, word: u32, ext: u32) -> Result<String, String> {
    if name.contains("f16") || name == "v_sat_pk_u8_i16" && word & 511 == 0xf9 {
        return Err(format!("{name}: unsupported SDWA/DPP half operand"));
    }
    let kind = word & 511;
    let d = word >> 17 & 255;
    let reg = ext & 255;
    let breg = word >> 9 & 255;
    let unary = family == "VOP1";
    let compare = family == "VOPC";
    let expr = |a: &str, b: &str| {
        if unary {
            vector_unary::expression(name, a)
        } else {
            vector_binary::expression(name, a, b, d)
        }
    };
    let (src_float, dst_float) = float_types(name);
    if kind == 0xf9 {
        let operand = |code: u32, flags: u32| -> Result<String, String> {
            let val = if flags & 128 != 0 {
                source(code, None).map_err(|e| format!("{name}: {e}"))?
            } else {
                format!("v[{code}]")
            };
            let value = extract(val, flags & 7, !src_float && flags & 8 != 0);
            Ok(float_mods(
                value,
                src_float && flags & 32 != 0,
                src_float && flags & 16 != 0,
            ))
        };
        let a = operand(reg, ext >> 16)?;
        let b = if unary {
            "0u".into()
        } else {
            operand(breg, ext >> 24)?
        };
        if compare {
            let dst = if name.starts_with("v_cmpx_") {
                126
            } else if ext & (1 << 15) != 0 {
                ext >> 8 & 127
            } else {
                106
            };
            let dst = if dst == 125 {
                "s_null".into()
            } else {
                format!("s[{dst}]")
            };
            let cond = vector_compare::condition(name, reg, &a, &b)?;
            return Ok(format!(
                "{{ bool t = ({ACTIVE}) && ({cond}); {dst} = Ballot(lane, t); }}"
            ));
        }
        let mut value = expr(&a, &b)?;
        let omod = (ext >> 14) & 3;
        let clamp = ext & (1 << 13) != 0;
        if clamp || omod != 0 {
            if !dst_float {
                return Err(format!("{name}: integer SDWA clamp or omod"));
            }
            let scale = ["1.0f", "2.0f", "4.0f", "0.5f"][omod as usize];
            let scaled = format!("asfloat({value}) * {scale}");
            value = if clamp {
                format!("asuint(saturate({scaled}))")
            } else {
                format!("asuint({scaled})")
            };
        }
        let placed = place(value, d, ext >> 8 & 7, ext >> 11 & 3);
        return Ok(format!("if ({ACTIVE}) v[{d}] = {placed};"));
    }
    if compare {
        return Err(format!("{name}: DPP comparison unsupported"));
    }
    let b = if unary {
        "0u".into()
    } else {
        format!("v[{breg}]")
    };
    if kind == 0xfa {
        let (sl, valid) = row(ext >> 8 & 511).map_err(|e| format!("{name}: {e}"))?;
        let a = float_mods(
            "dpp_a".into(),
            src_float && ext & (1 << 21) != 0,
            src_float && ext & (1 << 20) != 0,
        );
        let b = float_mods(
            b,
            src_float && ext & (1 << 23) != 0,
            src_float && ext & (1 << 22) != 0,
        );
        let value = expr(&a, &b)?;
        let ok = if ext & (1 << 18) != 0 {
            format!("({valid})")
        } else {
            format!("(({valid}) && (((s[126] >> dpp_sl) & 1u) != 0u))")
        };
        let masks = format!(
            "(((0x{:X}u >> (lane >> 4u)) & 1u) != 0u && ((0x{:X}u >> ((lane & 15u) >> 2u)) & 1u) != 0u)",
            ext >> 28,
            ext >> 24 & 15
        );
        let guard = if ext & (1 << 19) != 0 {
            ""
        } else {
            " && dpp_ok"
        };
        return Ok(format!(
            "{{ uint dpp_sl = {sl}; uint dpp_raw = ReadLane(lane, v[{reg}], {valid} ? dpp_sl : lane); bool dpp_ok = {ok}; uint dpp_a = dpp_ok ? dpp_raw : 0u; if (({ACTIVE}) && ({masks}){guard}) v[{d}] = {value}; }}"
        ));
    }
    let cond = if kind == 0xea {
        ""
    } else {
        " && (((s[126] >> dpp_sl) & 1u) != 0u)"
    };
    let value = expr("dpp_a", &b)?;
    Ok(format!(
        "{{ uint dpp_sl = ((lane & ~7u) | ((0x{:X}u >> (3u * (lane & 7u))) & 7u)); uint dpp_a = ReadLane(lane, v[{reg}], dpp_sl); if (({ACTIVE}){cond}) v[{d}] = {value}; }}",
        ext >> 8
    ))
}

fn float_types(name: &str) -> (bool, bool) {
    let mut parts = name.rsplit('_');
    let src = parts.next() == Some("f32");
    let dst = parts.next().unwrap_or("");
    let ty = dst
        .strip_prefix("ubyte")
        .or_else(|| dst.strip_prefix(['f', 'i', 'u', 'b']));
    (
        src,
        if ty.is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) {
            dst == "f32"
        } else {
            src
        },
    )
}
