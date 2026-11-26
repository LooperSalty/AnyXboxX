#![cfg(windows)]

use std::process::Command;

#[test]
fn usage_and_missing_target_are_reported_before_loading_the_runtime() {
    let binary = env!("CARGO_BIN_EXE_anyxboxx-native-launcher");
    let usage = Command::new(binary).output().unwrap();
    assert_eq!(usage.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&usage.stderr).contains("Usage : anyxboxx.exe"));
    let missing = Command::new(binary)
        .arg("AnyXboxX_missing_test_target.exe")
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("AnyXboxX_missing_test_target.exe"));
}
