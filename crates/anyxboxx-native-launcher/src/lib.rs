//! Components of the native launcher port. The C++ launcher remains the runtime entry point.

#[cfg(windows)]
pub mod command_line;
#[cfg(windows)]
pub mod exceptions;
#[cfg(windows)]
pub mod execution;
#[cfg(windows)]
pub mod hooks;
pub mod image;
pub mod imports;
#[cfg(windows)]
pub mod libraries;
#[cfg(windows)]
pub mod memory;
#[cfg(windows)]
pub mod patch;
pub mod redirect;
#[cfg(windows)]
pub mod registration;
#[cfg(windows)]
pub mod resolver;

/// Extract the executable name using the native launcher's manifest scanning rules.
///
/// The caller must limit the input to the first MiB, as the C++ launcher does. The returned
/// bytes are in the system ANSI code page; conversion to Windows paths belongs to the caller.
pub fn manifest_executable(input: &[u8]) -> Option<Vec<u8>> {
    let text: Vec<u8> = input.iter().copied().filter(|&byte| byte != 0).collect();
    let tag = find(&text, b"<Executable ")?;
    let name = tag + find(&text[tag..], b"Name=\"")? + 6;
    let end = text[name..]
        .iter()
        .position(|&byte| byte == b'"')
        .map_or(text.len(), |offset| name + offset);
    let executable = &text[name..end];
    if find(executable, b"..").is_some() || executable.contains(&b':') {
        return None;
    }
    Some(executable.to_vec())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|part| part == needle)
}

#[cfg(test)]
mod tests {
    use super::manifest_executable;

    #[test]
    fn reads_ascii_and_zero_interleaved_manifests() {
        let text = b"<Executable Name=\"bin\\test.exe\">";
        let wide: Vec<_> = text.iter().flat_map(|&byte| [byte, 0]).collect();
        for input in [text.as_slice(), wide.as_slice()] {
            assert_eq!(
                manifest_executable(input).as_deref(),
                Some(&b"bin\\test.exe"[..])
            );
        }
    }

    #[test]
    fn rejects_missing_or_escaping_names() {
        for text in [
            b"<Executable>".as_slice(),
            b"<Executable Name=\"../test.exe\">",
            b"<Executable Name=\"C:test.exe\">",
        ] {
            assert_eq!(manifest_executable(text), None);
        }
    }

    #[test]
    fn matches_the_original_unterminated_attribute_behavior() {
        assert_eq!(
            manifest_executable(b"<Executable Name=\"test.exe"),
            Some(b"test.exe".to_vec())
        );
    }
}
