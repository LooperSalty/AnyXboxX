#![cfg(windows)]

use std::process::{Command, Output};

fn check() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_victory"));
    cmd.arg("upscaler-check")
        .env_remove("VICTORY_DLSS_MODE")
        .env_remove("VICTORY_DLSS_PRESET")
        .env_remove("VICTORY_STREAMLINE_PRESENT")
        .env_remove("VICTORY_CHECK_TEARDOWN");
    cmd
}

fn text(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

#[test]
fn missing_sdk_fails_for_all_three_checks() {
    let empty = tempfile::tempdir().unwrap();
    for flags in [&[][..], &["--device"], &["--present"]] {
        let output = check().env("VICTORY_STREAMLINE_DIR", empty.path()).args(flags).output().unwrap();
        let report = text(&output);
        assert!(!output.status.success() && report.contains("missing sl.interposer.dll"), "{report}");
        assert!(!report.contains("DLSS ran"), "{report}");
    }
}

#[test]
fn rejects_invalid_output_before_loading() {
    for size in ["0x360", "640", "640x4294967295"] {
        let output = check().args(["--device", "--output", size]).output().unwrap();
        assert!(!output.status.success() && text(&output).contains("output"));
    }
}

#[test]
#[ignore = "needs signed Streamline DLLs and a DLSS-capable GPU"]
fn dlss_runs_three_times_on_the_device() {
    let output = check().arg("--device").output().unwrap();
    let report = text(&output);
    assert!(output.status.success(), "{report}");
    assert!(report.contains("DLSS ran three times") && report.contains("Streamline shut down: ok (0)"), "{report}");
}

#[test]
#[ignore = "needs signed Streamline DLLs, a frame generation GPU and an interactive desktop"]
fn present_also_checks_the_device_and_counts_generated_frames() {
    let output = check().arg("--present").output().unwrap();
    let report = text(&output);
    assert!(output.status.success(), "{report}");
    assert!(report.contains("DLSS ran three times") && report.contains("frames shown by it"), "{report}");
    assert!(report.contains("Streamline shut down: ok (0)"), "{report}");
}
