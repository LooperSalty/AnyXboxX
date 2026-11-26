//! What the landing page shows and what its big button does, decided from plain facts.

use std::path::Path;

use crate::app::Verify;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// The folder is checked and whole.
    Ready,
    /// No game folder typed yet.
    NeedsFolder,
    /// A folder is typed but not checked, or the check is running.
    NeedsCheck,
    /// The check found missing or damaged files, or could not run.
    Problem,
}

pub fn folder_readiness(has_path: bool, verify: &Verify) -> Readiness {
    if !has_path {
        return Readiness::NeedsFolder;
    }
    match verify {
        Verify::Done(report) if report.is_ok() => Readiness::Ready,
        Verify::Done(_) | Verify::Failed(_) => Readiness::Problem,
        Verify::NotRun | Verify::Running => Readiness::NeedsCheck,
    }
}

/// The text of the hero button.
pub fn play_label(readiness: Readiness, busy: bool, verifying: bool) -> &'static str {
    if verifying {
        return "Checking the game folder...";
    }
    if busy {
        return "Launching...";
    }
    match readiness {
        Readiness::Ready => "PLAY",
        Readiness::NeedsFolder => "Choose the game folder",
        Readiness::NeedsCheck => "Check the game folder",
        Readiness::Problem => "Fix the game folder",
    }
}

/// Whether the shader cache folder exists and holds something.
pub fn cache_filled(dir: Option<&Path>) -> bool {
    dir.and_then(|d| std::fs::read_dir(d).ok())
        .is_some_and(|mut entries| entries.next().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_follows_the_folder_and_its_check() {
        assert_eq!(
            folder_readiness(false, &Verify::NotRun),
            Readiness::NeedsFolder
        );
        assert_eq!(
            folder_readiness(true, &Verify::NotRun),
            Readiness::NeedsCheck
        );
        assert_eq!(
            folder_readiness(true, &Verify::Running),
            Readiness::NeedsCheck
        );
        assert_eq!(
            folder_readiness(true, &Verify::Failed("x".into())),
            Readiness::Problem
        );
    }

    #[test]
    fn the_button_says_what_comes_next() {
        assert_eq!(play_label(Readiness::Ready, false, false), "PLAY");
        assert_eq!(play_label(Readiness::Ready, true, false), "Launching...");
        assert_eq!(
            play_label(Readiness::NeedsFolder, false, false),
            "Choose the game folder"
        );
        assert_eq!(
            play_label(Readiness::NeedsCheck, true, true),
            "Checking the game folder..."
        );
    }

    #[test]
    fn the_cache_counts_only_when_it_holds_something() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert!(!cache_filled(Some(dir.path())));
        std::fs::write(dir.path().join("a.bin"), b"x").expect("write");
        assert!(cache_filled(Some(dir.path())));
        assert!(!cache_filled(Some(&dir.path().join("missing"))));
        assert!(!cache_filled(None));
    }
}
