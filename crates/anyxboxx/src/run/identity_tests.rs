use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{IdentityError, RunError, Victory};

const IMAGE: &[u8] = b"MZ\0\0inert verification fixture";

struct Fixture {
    dir: tempfile::TempDir,
    exe: PathBuf,
    sidecar: PathBuf,
    manifest: Value,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let artifact_dir = dir.path().join("build output ñ");
        fs::create_dir(&artifact_dir).unwrap();
        let exe = artifact_dir.join("victory.exe");
        fs::write(&exe, IMAGE).unwrap();
        Self {
            dir,
            exe,
            sidecar: artifact_dir.join("victory.exe.identity.json"),
            manifest: json!({
                "schema": 1,
                "executable_sha256": digest(IMAGE),
                "executable_size": IMAGE.len(),
                "source_commit": "a".repeat(40),
                "tombox_gitlink": "b".repeat(40),
                "target": "x86_64-pc-windows-msvc",
                "profile": "release",
                "translator_version": 2,
            }),
        }
    }

    fn pin(&self) -> String {
        self.pin_bytes(&serde_json::to_vec(&self.manifest).unwrap())
    }

    fn pin_bytes(&self, bytes: &[u8]) -> String {
        fs::write(&self.sidecar, bytes).unwrap();
        digest(bytes)
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn verified_prebuilt_plans_warm_and_run_without_cargo() {
    let fixture = Fixture::new();
    let pin = fixture.pin();
    let victory = Victory::pinned_prebuilt(&fixture.exe, &pin).unwrap();
    assert!(victory.build_command().is_none());
    assert_eq!(victory.manifest_sha256(), Some(pin.as_str()));
    let game = victory.game_command(fixture.dir.path(), &vec![]);
    let warm = victory.warm_command(&[], &vec![]);
    let path = fixture.exe.canonicalize().unwrap();
    assert_eq!(Path::new(game.get_program()), path);
    assert_eq!(Path::new(warm.get_program()), path);
    let line = game.identity_line().unwrap();
    assert_eq!(warm.identity_line().as_deref(), Some(line.as_str()));
    let record: Value =
        serde_json::from_str(line.strip_prefix("VICTORY_RUNTIME_IDENTITY=").unwrap()).unwrap();
    assert_eq!(record["manifest_sha256"], pin);
    assert_eq!(record["trust"], "pinned-manifest-sha256");
    for (key, value) in fixture.manifest.as_object().unwrap() {
        assert_eq!(record[key], *value);
    }
    assert!(matches!(
        victory.check_diagnostics(true),
        Err(RunError::PrebuiltIdentity(IdentityError::Diagnostics))
    ));
}

#[test]
fn valid_replacement_manifest_also_needs_the_trusted_pin() {
    let mut fixture = Fixture::new();
    let old_pin = fixture.pin();
    fixture.manifest["source_commit"] = json!("c".repeat(40));
    let new_pin = fixture.pin();
    assert_ne!(old_pin, new_pin);
    assert!(matches!(
        Victory::pinned_prebuilt(&fixture.exe, &old_pin),
        Err(RunError::PrebuiltIdentity(IdentityError::ManifestDigest))
    ));
    let new = Victory::pinned_prebuilt(&fixture.exe, &new_pin).unwrap();
    assert_eq!(new.manifest_sha256(), Some(new_pin.as_str()));
}

#[test]
fn rejects_executable_changes_even_when_the_size_matches() {
    let fixture = Fixture::new();
    let pin = fixture.pin();
    fs::write(&fixture.exe, vec![0; IMAGE.len()]).unwrap();
    assert!(matches!(
        Victory::pinned_prebuilt(&fixture.exe, &pin),
        Err(RunError::PrebuiltIdentity(IdentityError::ExecutableDigest))
    ));
}

#[test]
fn validates_manifest_fields_and_schema_even_with_a_valid_pin() {
    for (key, invalid) in [
        ("target", json!("aarch64-pc-windows-msvc")),
        ("source_commit", json!("a445308")),
        ("tombox_gitlink", json!("0".repeat(40))),
        ("executable_sha256", json!("z".repeat(64))),
        ("executable_size", json!(0)),
    ] {
        let mut fixture = Fixture::new();
        fixture.manifest[key] = invalid;
        assert!(
            matches!(Victory::pinned_prebuilt(&fixture.exe, &fixture.pin()), Err(RunError::PrebuiltIdentity(IdentityError::Field(field))) if field == key)
        );
    }

    let mut fixture = Fixture::new();
    fixture.manifest["schema"] = json!(2);
    assert!(matches!(
        Victory::pinned_prebuilt(&fixture.exe, &fixture.pin()),
        Err(RunError::PrebuiltIdentity(IdentityError::Schema(2)))
    ));
}

#[test]
fn rejects_duplicate_unknown_and_oversized_manifest_metadata() {
    let fixture = Fixture::new();
    let manifest = serde_json::to_string(&fixture.manifest).unwrap();
    for mutation in ["\"schema\":1,", "\"unrecognized\":0,"] {
        let bytes = format!("{{{mutation}{}", &manifest[1..]);
        let pin = fixture.pin_bytes(bytes.as_bytes());
        assert!(matches!(
            Victory::pinned_prebuilt(&fixture.exe, &pin),
            Err(RunError::PrebuiltIdentity(IdentityError::ManifestJson(_)))
        ));
    }
    let pin = fixture.pin_bytes(&vec![b' '; 4097]);
    assert!(matches!(
        Victory::pinned_prebuilt(&fixture.exe, &pin),
        Err(RunError::PrebuiltIdentity(IdentityError::ManifestTooLarge(
            _
        )))
    ));
}

#[test]
fn command_keeps_the_executable_and_parent_protected_after_planning() {
    let fixture = Fixture::new();
    let victory = Victory::pinned_prebuilt(&fixture.exe, &fixture.pin()).unwrap();
    let command = victory.game_command(fixture.dir.path(), &vec![]);
    drop(victory);

    let moved_exe = fixture.exe.with_file_name("moved.exe");
    let parent = fixture.exe.parent().unwrap();
    let moved_parent = fixture.dir.path().join("moved");
    assert!(fs::write(&fixture.exe, b"modified").is_err());
    assert!(fs::rename(&fixture.exe, &moved_exe).is_err());
    assert!(fs::rename(parent, &moved_parent).is_err());

    drop(command);
    fs::rename(&fixture.exe, &moved_exe).unwrap();
    fs::rename(parent, moved_parent).unwrap();
}
