//! Load expressions shared by GLOBAL, FLAT and MUBUF.

fn half(width: &str, address: &str) -> String {
    let value = format!(
        "(((g_memory.Load(({address}) & ~3u) >> ((({address}) & 3u) * 8u)) | (((({address}) & 3u) == 3u ? g_memory.Load((({address}) & ~3u) + 4u) : 0u) << 8u)) & 0xFFFFu)"
    );
    if width == "sshort" {
        format!("asuint((asint({value}) << 16) >> 16)")
    } else {
        value
    }
}

pub fn statement(width: &str, count: u64, data: u64, address: &str, inside: &str) -> String {
    let prefix = if count > 1 {
        format!("uint a = {address}; ")
    } else {
        String::new()
    };
    let address = if count > 1 { "a" } else { address };
    let loads = if matches!(width, "ushort" | "sshort") {
        format!("v[{data}] = {};", half(width, address))
    } else if matches!(width, "ubyte" | "sbyte") {
        let value =
            format!("((g_memory.Load(({address}) & ~3u) >> ((({address}) & 3u) * 8u)) & 0xFFu)");
        let value = if width == "sbyte" {
            format!("asuint((asint({value}) << 24) >> 24)")
        } else {
            value
        };
        format!("v[{data}] = {value};")
    } else {
        (0..count)
            .map(|i| format!("v[{}] = g_memory.Load(({address}) + {}u);", data + i, 4 * i))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let zeros = (0..count)
        .map(|i| format!("v[{}] = 0u;", data + i))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "if (((s[126] >> lane) & 1u) != 0) {{ if ({inside}) {{ {prefix}{loads} }} else {{ {zeros} }} }}"
    )
}
