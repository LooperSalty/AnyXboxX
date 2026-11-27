"""Use small repositories and inert executables to check the release identity pipeline."""

import hashlib
import json
import shutil
import subprocess

import pytest

from tools import build_victory_identity as identity


def git(repo, *args):
    return identity.git(repo, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", *args)


@pytest.fixture
def checkout(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    git(repo, "init", "-q")
    tombox = repo / identity.TOMBOX
    tombox.mkdir(parents=True)
    git(tombox, "init", "-q")
    (tombox / "fixture.txt").write_text("source", encoding="utf-8")
    git(tombox, "add", "--", "fixture.txt")
    git(tombox, "commit", "-qm", "fixture: subproject")

    victory = repo / identity.VICTORY
    shader = victory / "crates/victory-shader/src"
    shader.mkdir(parents=True)
    (shader / "lib.rs").write_text("pub const TRANSLATOR_VERSION: u32 = 9;\n", encoding="utf-8")
    for folder in (repo, victory):
        (folder / "Cargo.lock").write_text("version = 4\n", encoding="utf-8")
        (folder / "rust-toolchain.toml").write_text(
            f'[toolchain]\nchannel = "1.98.1"\ntargets = ["{identity.TARGET}"]\n', encoding="utf-8",
        )
    git(repo, "add", "--all")
    git(repo, "commit", "-qm", "fixture: workspace")
    return repo


def test_checkout_records_both_lockfiles_and_identity_without_launching(checkout, tmp_path):
    snapshot = identity.read_checkout(checkout)
    assert snapshot.source_commit == git(checkout, "rev-parse", "HEAD")
    assert snapshot.tombox_gitlink == git(checkout / identity.TOMBOX, "rev-parse", "HEAD")
    assert snapshot.victory_tree == git(checkout, "rev-parse", "HEAD:third_party/victory")
    assert snapshot.toolchain == "1.98.1"
    assert snapshot.translator_version == 9
    for lock in identity.LOCKS:
        expected = hashlib.sha256((checkout / lock).read_bytes()).hexdigest()
        assert snapshot.lock_sha256[lock.as_posix()] == expected

    fixture = tmp_path / "victory.exe"
    fixture.write_bytes(b"MZ\0\0inert fixture")
    pin = identity.write_identity(fixture, snapshot, {"windows_sdk": "test"}, "release")
    encoded = fixture.with_suffix(".exe.identity.json").read_bytes()
    assert hashlib.sha256(encoded).hexdigest() == pin
    manifest = json.loads(encoded)
    assert manifest == {
        "schema": 1, "executable_sha256": identity.sha256(fixture),
        "executable_size": fixture.stat().st_size, "source_commit": snapshot.source_commit,
        "tombox_gitlink": snapshot.tombox_gitlink, "target": identity.TARGET,
        "profile": "release", "translator_version": 9,
    }
    evidence = json.loads(fixture.with_suffix(".exe.build.json").read_bytes())
    assert evidence["manifest_sha256"] == pin
    assert evidence["lock_sha256"] == snapshot.lock_sha256


def test_incompatible_submodule_commit_is_refused(checkout):
    git(checkout / identity.TOMBOX, "commit", "--allow-empty", "-qm", "fixture: drift")
    with pytest.raises(identity.BuildIdentityError, match="HEAD does not match"):
        identity.read_checkout(checkout)


def test_dirty_lockfile_is_refused(checkout):
    (checkout / identity.VICTORY / "Cargo.lock").write_text("version = 3\n", encoding="utf-8")
    with pytest.raises(identity.BuildIdentityError, match="clean checkout"):
        identity.read_checkout(checkout)


def test_different_workspace_toolchain_pins_are_refused(checkout):
    pin = identity.VICTORY / "rust-toolchain.toml"
    path = checkout / pin
    path.write_text(path.read_text(encoding="utf-8").replace("1.98.1", "1.98.2"), encoding="utf-8")
    git(checkout, "add", "--", pin.as_posix())
    git(checkout, "commit", "-qm", "fixture: change pin")
    with pytest.raises(identity.BuildIdentityError, match="pins disagree"):
        identity.read_checkout(checkout)


@pytest.mark.parametrize("profile", ["release", "diag"])
def test_build_command_pins_cargo_lock_target_and_profile(tmp_path, profile):
    target_dir = tmp_path / "fresh build"
    command = identity.build_command(tmp_path, target_dir, "1.98.1", profile, offline=True)
    assert command[:4] == ["cargo", "+1.98.1", "build", "--locked"]
    assert command[command.index("--target") + 1] == identity.TARGET
    assert command[command.index("--target-dir") + 1] == str(target_dir)
    assert command[command.index("--profile") + 1] == profile
    assert ("diagnostics" in command) == (profile == "diag")
    assert "--offline" in command


def test_exact_cargo_artifact_selection(tmp_path):
    target_dir = tmp_path / "target"
    target_dir.mkdir()
    exe = target_dir / "reported.exe"
    exe.write_bytes(b"MZ\0test")

    def message(path, kind="bin"):
        return json.dumps({
            "reason": "compiler-artifact", "target": {"name": "victory", "kind": [kind]},
            "executable": str(path),
        })

    assert identity.cargo_executable(f"not json\n{message(exe, 'lib')}\n{message(exe)}", target_dir) == exe
    with pytest.raises(identity.BuildIdentityError, match="reported 2"):
        identity.cargo_executable(f"{message(exe)}\n{message(exe)}", target_dir)
    other = tmp_path / "outside.exe"
    other.touch()
    with pytest.raises(identity.BuildIdentityError, match="outside"):
        identity.cargo_executable(message(other), target_dir)


def test_cargo_locked_refuses_a_manifest_with_stale_resolution(tmp_path):
    cargo = shutil.which("cargo")
    if cargo is None:
        pytest.skip("Cargo is required for the lock drift fixture")
    (tmp_path / "src").mkdir()
    (tmp_path / "src/main.rs").write_text("fn main() {}\n", encoding="utf-8")
    (tmp_path / "Cargo.toml").write_text('[package]\nname = "changed"\nversion = "0.1.0"\n', encoding="utf-8")
    old_lock = b'version = 4\n\n[[package]]\nname = "previous"\nversion = "0.1.0"\n'
    (tmp_path / "Cargo.lock").write_bytes(old_lock)
    result = subprocess.run(
        [cargo, "build", "--locked", "--offline"], cwd=tmp_path,
        capture_output=True, text=True, check=False,
    )
    assert result.returncode != 0 and "--locked" in result.stderr
    assert (tmp_path / "Cargo.lock").read_bytes() == old_lock
