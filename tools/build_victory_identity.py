"""Build VIctory from a clean, pinned checkout and record the selected artifact's identity."""

import argparse
import hashlib
import json
import os
import re
import subprocess
import tempfile
import tomllib
from dataclasses import asdict, dataclass
from pathlib import Path

VICTORY = Path("third_party/victory")
TOMBOX = Path("third_party/tombox")
TARGET = "x86_64-pc-windows-msvc"
LOCKS = (Path("Cargo.lock"), VICTORY / "Cargo.lock")


class BuildIdentityError(RuntimeError):
    """The checkout, toolchain or artifact could not be verified."""


def capture(command, cwd, env=None, *, also_accept=()):
    result = subprocess.run(
        command, cwd=cwd, env=env, stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace",
    )
    if result.returncode not in also_accept:
        result.check_returncode()
    return result.stdout.strip()


def git(repo, *args):
    return capture(["git", "-C", str(repo), *args], repo)


def sha256(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


@dataclass(frozen=True)
class Checkout:
    source_commit: str
    tombox_gitlink: str
    victory_tree: str
    toolchain: str
    translator_version: int
    lock_sha256: dict[str, str]


def read_checkout(repo):
    source_commit = git(repo, "rev-parse", "--verify", "HEAD^{commit}")
    gitlink = git(repo, "ls-tree", "HEAD", "--", TOMBOX.as_posix())
    match = re.fullmatch(r"160000 commit ([0-9a-f]{40})\tthird_party/tombox", gitlink)
    if match is None or not (repo / TOMBOX / ".git").exists():
        raise BuildIdentityError("TomBox must be initialized at the committed gitlink")
    if git(repo / TOMBOX, "rev-parse", "HEAD") != match[1]:
        raise BuildIdentityError("TomBox HEAD does not match the committed gitlink")
    if git(repo, "status", "--porcelain=v1", "--untracked-files=normal", "--ignore-submodules=none"):
        raise BuildIdentityError("the release build requires a clean checkout and submodule")

    pins = []
    for folder in (repo, repo / VICTORY):
        pin = tomllib.loads((folder / "rust-toolchain.toml").read_text(encoding="utf-8"))["toolchain"]
        if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", pin["channel"]) or TARGET not in pin["targets"]:
            raise BuildIdentityError(f"the toolchain must pin an exact Rust release and {TARGET}")
        pins.append(pin["channel"])
    if pins[0] != pins[1]:
        raise BuildIdentityError("the root and VIctory Rust toolchain pins disagree")

    shader = repo / VICTORY / "crates/victory-shader/src/lib.rs"
    versions = re.findall(r"^pub const TRANSLATOR_VERSION: u32 = ([0-9]+);$", shader.read_text(encoding="utf-8"), re.M)
    if len(versions) != 1 or not 0 < int(versions[0]) <= 0xFFFF_FFFF:
        raise BuildIdentityError("could not identify the shader translator version")
    victory_tree = git(repo, "rev-parse", f"HEAD:{VICTORY.as_posix()}")
    locks = {lock.as_posix(): sha256(repo / lock) for lock in LOCKS}
    return Checkout(source_commit, match[1], victory_tree, pins[0], int(versions[0]), locks)


def build_command(repo, target_dir, toolchain, profile, *, offline=False):
    command = [
        "cargo", f"+{toolchain}", "build", "--locked", "--target", TARGET,
        "--target-dir", str(target_dir), "--manifest-path", str(repo / VICTORY / "Cargo.toml"),
        "-p", "victory", "--profile", profile, "--message-format=json-render-diagnostics",
    ]
    if profile == "diag":
        command += ["--features", "diagnostics"]
    if offline:
        command.append("--offline")
    return command


def configured_tools(repo, toolchain):
    env = os.environ.copy()
    vc_root = os.environ.get("VCToolsInstallDir")
    sdk_root = os.environ.get("WindowsSdkDir")
    sdk_version = os.environ.get("WindowsSDKVersion", "").rstrip("\\/")
    if not vc_root or not sdk_root or not sdk_version:
        raise BuildIdentityError("use a Visual Studio x64 developer shell with the Windows SDK")

    vc_bin = Path(vc_root) / "bin/Hostx64/x64"
    cl = vc_bin / "cl.exe"
    linker = vc_bin / "link.exe"
    dxc = Path(sdk_root) / "bin" / sdk_version / "x64/dxc.exe"
    env.update({
        "CC": str(cl),
        "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER": str(linker),
        "RUSTC": capture(["rustup", "which", "--toolchain", toolchain, "rustc"], repo),
        "RUSTC_WRAPPER": "",
        "RUSTC_WORKSPACE_WRAPPER": "",
        "CARGO_ENCODED_RUSTFLAGS": "",
    })
    rust = {
        name: capture([name, f"+{toolchain}", "-Vv"], repo, env)
        for name in ("rustc", "cargo")
    }
    for name, version in rust.items():
        if f"release: {toolchain}" not in version.splitlines() or f"host: {TARGET}" not in version.splitlines():
            raise BuildIdentityError(f"{name} does not match the pinned Windows x64 toolchain")
    tools = {}
    # link.exe prints its version and help with exit code 1100 when it has no input objects.
    for tool, argument, help_codes in ((cl, "/?", ()), (linker, "/?", (1100,)), (dxc, "--version", ())):
        lines = capture([str(tool), argument], repo, env, also_accept=help_codes).splitlines()
        if not lines:
            raise BuildIdentityError(f"{tool.name} did not report its version")
        tools[tool.stem] = {"version": lines[0], "sha256": sha256(tool)}
    return env, {**rust, "windows_sdk": sdk_version, "tools": tools}


def cargo_executable(messages, target_dir):
    artifacts = []
    for line in messages.splitlines():
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if not isinstance(message, dict) or message.get("reason") != "compiler-artifact":
            continue
        target = message.get("target", {})
        exe = message.get("executable")
        kinds = target.get("kind") if isinstance(target, dict) else None
        if (isinstance(kinds, list) and "bin" in kinds and target.get("name") == "victory"
                and isinstance(exe, str) and exe):
            artifacts.append(Path(exe))
    if len(artifacts) != 1:
        raise BuildIdentityError(f"Cargo reported {len(artifacts)} VIctory binaries, expected one")
    exe = artifacts[0].resolve(strict=True)
    if not (artifacts[0].is_absolute() and exe.is_relative_to(target_dir)
            and exe.is_file() and exe.suffix.lower() == ".exe"):
        raise BuildIdentityError("Cargo executable is outside the fresh target directory")
    return exe


def json_bytes(value):
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")


def publish(path, data):
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as output:
            temporary = Path(output.name)
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        temporary = None
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def write_identity(exe, checkout, tools, profile):
    manifest = json_bytes({
        "schema": 1,
        "executable_sha256": sha256(exe),
        "executable_size": exe.stat().st_size,
        "source_commit": checkout.source_commit,
        "tombox_gitlink": checkout.tombox_gitlink,
        "target": TARGET,
        "profile": profile,
        "translator_version": checkout.translator_version,
    })
    pin = hashlib.sha256(manifest).hexdigest()
    evidence = {"schema": 1, "manifest_sha256": pin, **asdict(checkout), **tools, "target": TARGET, "profile": profile}
    publish(exe.with_name(exe.name + ".build.json"), json_bytes(evidence))
    publish(exe.with_name(exe.name + ".identity.json"), manifest)
    return pin


def build(repo, target_dir, profile, *, offline):
    checkout = read_checkout(repo)
    env, versions = configured_tools(repo, checkout.toolchain)
    target_dir.mkdir(parents=True, exist_ok=False)
    result = subprocess.run(
        build_command(repo, target_dir, checkout.toolchain, profile, offline=offline),
        cwd=repo / VICTORY, env=env, check=True, stdout=subprocess.PIPE,
    )
    exe = cargo_executable(result.stdout, target_dir)
    if read_checkout(repo) != checkout:
        raise BuildIdentityError("build inputs changed while Cargo was running")
    return exe, write_identity(exe, checkout, versions, profile)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--target-dir", required=True, type=Path, help="new output directory for this build")
    parser.add_argument("--profile", choices=("release", "diag"), default="release")
    parser.add_argument("--offline", action="store_true")
    args = parser.parse_args(argv)
    try:
        exe, pin = build(args.repo.resolve(), args.target_dir.resolve(), args.profile, offline=args.offline)
    except (BuildIdentityError, OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"VIctory build identity: {error}\n")
    print(f"Cargo executable: {exe}")
    print(f"Manifest SHA-256: {pin}")


if __name__ == "__main__":
    main()
