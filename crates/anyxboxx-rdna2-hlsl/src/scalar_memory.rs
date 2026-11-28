//! Straight-line SMEM loads and stores, matching the Python emitter.

pub fn statements(name: &str, word: u64) -> Result<Vec<String>, String> {
    let (buffer, rest) = if let Some(rest) = name.strip_prefix("s_buffer_") {
        (true, rest)
    } else if let Some(rest) = name.strip_prefix("s_") {
        (false, rest)
    } else {
        return Err(format!("untranslated instruction: {name}"));
    };
    if let Some(kind) = rest.strip_prefix("atomic_") {
        return crate::scalar_atomic::statements(name, kind, buffer, word);
    }
    let (store, width) = if let Some(width) = rest.strip_prefix("load_") {
        (false, width)
    } else if let Some(width) = rest.strip_prefix("store_") {
        (true, width)
    } else {
        return Err(format!("untranslated instruction: {name}"));
    };
    let count = match width {
        "dword" => 1,
        "dwordx2" => 2,
        "dwordx4" => 4,
        "dwordx8" if !store => 8,
        "dwordx16" if !store => 16,
        _ => return Err(format!("untranslated instruction: {name}")),
    };
    let sdata = (word >> 6) & 0x7f;
    let sbase = (word & 0x3f) * 2;
    let raw = ((word >> 32) & 0x1f_ffff) as u32;
    let offset = ((raw << 11) as i32 >> 11) as u32;
    let soffset = (word >> 57) & 0x7f;
    let extra = if soffset == 125 {
        String::new()
    } else {
        format!(" + s[{soffset}]")
    };
    let off = format!("0x{offset:X}u{extra}");
    if store {
        let inside = if buffer {
            format!("(asint(o) >= 0 && o + {}u <= s[{}])", count * 4, sbase + 2)
        } else {
            "true".into()
        };
        let writes = (0..count)
            .map(|i| format!("g_memory.Store(a + {}u, s[{}]);", i * 4, sdata + i))
            .collect::<Vec<_>>()
            .join(" ");
        return Ok(vec![
            format!(
                "{{ uint o = {off}; uint a = s[{sbase}] + o; if (lane == 0u && {inside}) {{ {writes} }} }}"
            ),
            "AllMemoryBarrierWithGroupSync();".into(),
        ]);
    }
    if buffer {
        let register = if soffset == 125 {
            "0u".into()
        } else {
            format!("s[{soffset}]")
        };
        let valid = if (offset as i32) < 0 {
            format!("{register} >= {}u", offset.wrapping_neg())
        } else {
            format!("{register} <= 0x{:X}u", u32::MAX - offset)
        };
        let bound = format!("s[{}]", sbase + 2);
        let loads = (0..count)
            .map(|i| {
                format!(
                    "s[{}] = ok ? g_memory.Load(a + {}u) : 0u;",
                    sdata + i,
                    4 * i
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        Ok(vec![format!(
            "{{ uint o = {off}; uint a = s[{sbase}] + o; bool ok = {valid} && o <= {bound} && {}u <= {bound} - o; {loads} }}",
            4 * count
        )])
    } else {
        let loads = (0..count)
            .map(|i| format!("s[{}] = g_memory.Load(a + {}u);", sdata + i, i * 4))
            .collect::<Vec<_>>()
            .join(" ");
        Ok(vec![format!("{{ uint a = s[{sbase}] + {off}; {loads} }}")])
    }
}
