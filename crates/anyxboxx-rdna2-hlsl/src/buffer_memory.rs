//! Untyped MUBUF accesses with index and byte-offset VGPRs.
use crate::scalar::source;

pub fn statements(name: &str, word: u64) -> Result<Vec<String>, String> {
    let rest = name
        .strip_prefix("buffer_")
        .ok_or_else(|| format!("untranslated instruction: {name}"))?;
    let atomic = match rest {
        "atomic_add" => Some("InterlockedAdd"),
        "atomic_swap" => Some("InterlockedExchange"),
        "atomic_and" => Some("InterlockedAnd"),
        "atomic_or" => Some("InterlockedOr"),
        "atomic_xor" => Some("InterlockedXor"),
        "atomic_umin" => Some("InterlockedMin"),
        "atomic_umax" => Some("InterlockedMax"),
        _ => None,
    };
    let (write, width) = if atomic.is_some() {
        (false, "dword")
    } else if let Some(width) = rest.strip_prefix("load_") {
        (false, width)
    } else if let Some(width) = rest.strip_prefix("store_") {
        (true, width)
    } else {
        return Err(format!("untranslated instruction: {name}"));
    };
    let count = match width {
        "dword" => 1,
        "ubyte" | "sbyte" | "ushort" | "sshort" if !write => 1,
        "dwordx2" => 2,
        "dwordx3" => 3,
        "dwordx4" => 4,
        _ => return Err(format!("untranslated instruction: {name}")),
    };
    let idxen = (word >> 13) & 1 != 0;
    let offset = word & 0xfff;
    let offen = (word >> 12) & 1 != 0;
    let addr = (word >> 32) & 255;
    if idxen && offen && addr == 255 {
        return Err(format!("{name}: address pair past v255"));
    }
    let data = (word >> 40) & 255;
    let resource = ((word >> 48) & 31) * 4;
    let soffset = ((word >> 56) & 255) as u32;
    if (193..=208).contains(&soffset) {
        return Err(format!(
            "untranslated instruction: {name}: negative inline SOFFSET"
        ));
    }
    let index = if idxen {
        format!("v[{addr}]")
    } else {
        "0u".into()
    };
    let vector_offset = if offen {
        format!("v[{}]", addr + u64::from(idxen))
    } else {
        "0u".into()
    };
    let scalar_offset = source(soffset, None).map_err(|e| format!("{name}: {e}"))?;
    let size = if matches!(width, "ubyte" | "sbyte") {
        1
    } else if matches!(width, "ushort" | "sshort") {
        2
    } else {
        count * 4
    };
    let setup = format!(
        "uint addr; bool inside = BufferAddress(s[{resource}], s[{}], s[{}], {scalar_offset}, {index}, {vector_offset}, {offset}u, {size}u, addr);",
        resource + 1,
        resource + 2
    );
    let (access, barrier) = if let Some(function) = atomic {
        let keep = if (word >> 14) & 1 != 0 {
            format!(" v[{data}] = old;")
        } else {
            String::new()
        };
        (
            format!(
                "if ((((s[126] >> lane) & 1u) != 0) && inside) {{ uint old; g_memory.{function}(addr, v[{data}], old);{keep} }}"
            ),
            true,
        )
    } else if write {
        let writes = (0..count)
            .map(|i| format!("g_memory.Store(addr + {}u, v[{}]);", 4 * i, data + i))
            .collect::<Vec<_>>()
            .join(" ");
        (
            format!("if ((((s[126] >> lane) & 1u) != 0) && inside) {{ {writes} }}"),
            true,
        )
    } else {
        (
            crate::memory_load::statement(width, count, data, "addr", "inside"),
            false,
        )
    };
    let mut lines = vec![format!("{{ {setup} {access} }}")];
    if barrier {
        lines.push("AllMemoryBarrierWithGroupSync();".into());
    }
    Ok(lines)
}
