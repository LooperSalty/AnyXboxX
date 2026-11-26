//! Tells the severity of a log line, so the console drawer can colour it.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// The launcher's own step headers (`== Building VIctory ==`).
    Step,
    Error,
    Warn,
    Info,
}

pub fn classify_line(line: &str) -> Level {
    let trimmed = line.trim_start();
    let lower = trimmed.to_lowercase();
    if trimmed.starts_with("==") {
        Level::Step
    } else if trimmed.starts_with("!!")
        || ["error", "panicked", "failed", "fatal"]
            .iter()
            .any(|w| lower.contains(w))
    {
        Level::Error
    } else if lower.contains("warn") {
        Level::Warn
    } else {
        Level::Info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_get_their_level() {
        assert_eq!(classify_line("== Building VIctory =="), Level::Step);
        assert_eq!(classify_line("!! Could not run"), Level::Error);
        assert_eq!(
            classify_line("error[E0432]: unresolved import"),
            Level::Error
        );
        assert_eq!(classify_line("thread 'main' panicked at x"), Level::Error);
        assert_eq!(classify_line("warning: unused variable"), Level::Warn);
        assert_eq!(classify_line("Compiling victory v0.1.0"), Level::Info);
    }
}
