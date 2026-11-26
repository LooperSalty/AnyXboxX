//! Embeds the git commit of the build as `ANYXBOXX_GIT_COMMIT` (short hash, or `unknown` outside a
//! git checkout) for the About line of the window. It reruns when HEAD or the branch it names moves.

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

fn watch(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let commit = git(&["rev-parse", "--short=8", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=ANYXBOXX_GIT_COMMIT={commit}");
    if let Some(git_dir) = git(&["rev-parse", "--git-dir"]).map(PathBuf::from) {
        watch(&git_dir.join("HEAD"));
    }
    let common = git(&["rev-parse", "--git-common-dir"]).map(PathBuf::from);
    if let Some(common) = common {
        watch(&common.join("packed-refs"));
        if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
            watch(&common.join(branch));
        }
    }
}
