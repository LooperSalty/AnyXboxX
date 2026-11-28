//! Formatted MUBUF and MTBUF loads and stores. DXIL helpers implement the
//! runtime format lookup, component conversions and partial writes.
use anyxboxx_rdna2_ref::format_access::{Access, access, resolve};

use crate::scalar::source;

pub fn statements(name: &str, word: u64) -> Result<Vec<String>, String> {
    let a = access(name, word).map_err(|e| format!("{name}: {e}"))?;
    if a.typed {
        // MTBUF encodes its format in the instruction. Runtime descriptor
        // enablement is handled in HLSL independently of this format check.
        resolve(name, &a, &[0; 4]).map_err(|e| format!("{name}: {e}"))?;
    }
    let r = a.srsrc;
    let code = a
        .fixed
        .map_or_else(|| "((d.w >> 12) & 0x7Fu)".into(), |f| format!("{f}u"));
    let mut info = format!("BufFormatInfo({code}, d)");
    if a.d16 && !a.store {
        info = format!("BufD16Info({info})");
    }
    let soffset = match a.soffset {
        125 | 128 => String::new(),
        code => format!(
            " + {}",
            source(code, None).map_err(|e| format!("{name}: {e}"))?
        ),
    };
    let index = if a.idxen {
        format!("v[{}]", a.vaddr)
    } else {
        "0u".into()
    };
    let mut offset = format!("{}u", a.offset);
    if a.offen {
        offset.push_str(&format!(" + v[{}]", a.vaddr + usize::from(a.idxen)));
    }
    let enabled = if a.typed {
        " && ((d.w >> 12) & 0x7Fu) != 0u"
    } else {
        ""
    };
    let mut lines = vec![
        format!(
            "uint4 d = uint4(s[{r}], s[{}], s[{}], s[{}]);",
            r + 1,
            r + 2,
            r + 3
        ),
        format!("uint info = {info}, size = BufSize(info), stride = (d.y >> 16) & 0x3FFFu;"),
        format!("uint index = {index}, offset = {offset};"),
        format!(
            "bool inside = info != 0u{enabled} && (stride != 0u ? index < d.z : (offset >= {}u && offset <= d.z && size <= d.z - offset));",
            a.offset
        ),
        format!("uint address = d.x{soffset} + index * stride + offset;"),
    ];
    if a.store {
        store(&a, &mut lines);
    } else {
        load(&a, &mut lines);
    }
    let mut body = vec![format!(
        "if (((s[126] >> lane) & 1u) != 0) {{ {} }}",
        lines.join(" ")
    )];
    if a.store {
        body.push("AllMemoryBarrierWithGroupSync();".into());
    }
    Ok(body)
}

fn store(a: &Access, lines: &mut Vec<String>) {
    let values: Vec<_> = (0..4)
        .map(|i| {
            if i >= a.count {
                return "0u".into();
            }
            if !a.d16 {
                return format!("v[{}]", a.vdata + i);
            }
            let shift = 16 * ((i % 2) | usize::from(a.high));
            format!(
                "BufWiden(info, {i}u, (v[{}] >> {shift}u) & 0xFFFFu)",
                a.vdata + i / 2
            )
        })
        .collect();
    let allowed = if a.typed {
        "true".into()
    } else {
        format!("BufStoreSelectsOk(info, d.w, {}u, {})", a.count, a.d16)
    };
    lines.push(format!(
        "if (inside && {allowed}) BufWrite(address, size, BufEncode(info, uint4({})));",
        values.join(", ")
    ));
}

fn load(a: &Access, lines: &mut Vec<String>) {
    lines.push(format!("uint4 c = uint4(0u, 0u, 0u, 0u); if (inside) c = BufDecode(info, BufRead(address, size), {});", a.d16));
    for i in 0..a.count {
        let mut value = if a.typed {
            format!("c[{i}]")
        } else {
            format!("BufSelect(c, info, (d.w >> {}) & 7u)", 3 * i)
        };
        if a.d16 {
            value = format!("BufNarrow(info, {value})");
        }
        lines.push(format!("uint r{i} = {value};"));
    }
    for i in (0..a.count).step_by(if a.d16 { 2 } else { 1 }) {
        let reg = a.vdata + if a.d16 { i / 2 } else { i };
        let value = if !a.d16 {
            format!("r{i}")
        } else if i + 1 < a.count {
            format!("r{i} | (r{} << 16)", i + 1)
        } else {
            let (mask, shift) = if a.high {
                ("0xFFFFu", 16)
            } else {
                ("0xFFFF0000u", 0)
            };
            format!("(v[{reg}] & {mask}) | (r{i} << {shift}u)")
        };
        lines.push(format!("v[{reg}] = {value};"));
    }
}
