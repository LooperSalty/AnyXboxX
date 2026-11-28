//! Embeds the launcher version (`ANYXBOXX_LAUNCHER_VERSION`) and git commit
//! (`ANYXBOXX_GIT_COMMIT`, short hash or `unknown` outside a git checkout) for
//! the About line of the window. It reruns when HEAD, its branch, tags, or
//! `ANYXBOXX_LAUNCHER_VERSION` change.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

include!("src/version_norm.rs");

fn launcher_version() -> String {
    if let Ok(explicit) = std::env::var("ANYXBOXX_LAUNCHER_VERSION")
        && let Some(ver) = normalize_version(&explicit)
    {
        return ver.to_string();
    }
    for args in [
        &["describe", "--tags", "--exact-match", "--match", "launcher-v*", "HEAD"][..],
        &["describe", "--tags", "--abbrev=0", "--match", "launcher-v*", "HEAD"][..],
    ] {
        if let Some(tag) = git(args)
            && let Some(ver) = normalize_version(&tag)
        {
            return ver.to_string();
        }
    }
    env!("CARGO_PKG_VERSION").to_string()
}

fn watch(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/version_norm.rs");
    println!("cargo:rerun-if-env-changed=ANYXBOXX_LAUNCHER_VERSION");
    let version = launcher_version();
    println!("cargo:rustc-env=ANYXBOXX_LAUNCHER_VERSION={version}");
    let commit = git(&["rev-parse", "--short=8", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=ANYXBOXX_GIT_COMMIT={commit}");
    if let Some(git_dir) = git(&["rev-parse", "--git-dir"]).map(PathBuf::from) {
        watch(&git_dir.join("HEAD"));
    }
    let common = git(&["rev-parse", "--git-common-dir"]).map(PathBuf::from);
    if let Some(common) = common {
        watch(&common.join("packed-refs"));
        watch(&common.join("refs").join("tags"));
        if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
            watch(&common.join(branch));
        }
    }
}
