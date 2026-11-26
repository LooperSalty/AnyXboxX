//! VOPC integer comparisons for wave32.
use crate::scalar::source;

pub fn statement(name: &str, word: u32, literal: Option<u32>) -> Result<String, String> {
    let code = word & 0x1ff;
    if matches!(code, 0xe9 | 0xea | 0xf9 | 0xfa) {
        return Err(format!("untranslated instruction: {name} with SDWA or DPP"));
    }
    let a = if code >= 256 {
        format!("v[{}]", code - 256)
    } else {
        source(code, literal).map_err(|e| format!("{name}: {e}"))?
    };
    let b = format!("v[{}]", (word >> 9) & 0xff);
    let (prefix, suffix) = name
        .rsplit_once('_')
        .ok_or_else(|| format!("untranslated instruction: {name}"))?;
    let (op, target) = if let Some(op) = prefix.strip_prefix("v_cmpx_") {
        (op, "s[126]")
    } else if let Some(op) = prefix.strip_prefix("v_cmp_") {
        (op, "s[106]")
    } else {
        return Err(format!("untranslated instruction: {name}"));
    };
    if !matches!(suffix, "u32" | "i32" | "u16" | "i16") {
        return Err(format!("untranslated instruction: {name}"));
    }
    let condition = match op {
        "f" => "false".into(),
        "t" | "tru" => "true".into(),
        _ => {
            let symbol = match op {
                "lt" => "<",
                "eq" => "==",
                "le" => "<=",
                "gt" => ">",
                "lg" | "ne" | "neq" => "!=",
                "ge" => ">=",
                _ => return Err(format!("untranslated instruction: {name}")),
            };
            match suffix {
                "u16" => format!("(({a}) & 0xFFFFu) {symbol} (({b}) & 0xFFFFu)"),
                "i16" => {
                    format!("(asint(({a}) << 16u) >> 16) {symbol} (asint(({b}) << 16u) >> 16)")
                }
                "i32" => format!("asint({a}) {symbol} asint({b})"),
                _ => format!("({a}) {symbol} ({b})"),
            }
        }
    };
    Ok(format!(
        "{{ bool t = (((s[126] >> lane) & 1u) != 0) && ({condition}); {target} = Ballot(lane, t); }}"
    ))
}
