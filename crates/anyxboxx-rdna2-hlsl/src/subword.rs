//! Subword access expressions shared by GLOBAL, FLAT and MUBUF.

pub fn half(width: &str, address: &str) -> String {
    let value = format!(
        "(((g_memory.Load(({address}) & ~3u) >> ((({address}) & 3u) * 8u)) | (((({address}) & 3u) == 3u ? g_memory.Load((({address}) & ~3u) + 4u) : 0u) << 8u)) & 0xFFFFu)"
    );
    if width == "sshort" {
        format!("asuint((asint({value}) << 16) >> 16)")
    } else {
        value
    }
}
