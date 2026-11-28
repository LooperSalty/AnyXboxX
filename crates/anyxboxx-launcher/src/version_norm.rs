/// Normalises a launcher release tag or version string (`launcher-v0.2.1`, `v0.2.1`, `0.2.1`)
/// into a bare semver-like version string starting with an ASCII digit.
pub(crate) fn normalize_version(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    let stripped = trimmed
        .strip_prefix("launcher-v")
        .or_else(|| trimmed.strip_prefix('v'))
        .unwrap_or(trimmed);
    (!stripped.is_empty() && stripped.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .then_some(stripped)
}
