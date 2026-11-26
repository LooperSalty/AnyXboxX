"""Parity of the runtime DLLs ported to Rust (crates/anyxboxx-*) with the C++ ones (runtime/).

For each ported DLL, a runtime folder is assembled from the C++ build with that one DLL replaced by
the Rust cdylib, and the pytest files that check the C++ DLL run unchanged against it
(ANYXBOXX_RUNTIME points at the mixed folder). The C++ DLL is deleted only once this passes.
"""
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

import pefile
import pytest

from conftest import RUNTIME_DIR, runtime_built

REPO = Path(__file__).resolve().parents[1]
RUST_RELEASE = REPO / "target" / "release"
# DLL name in the runtime folder → (Rust cdylib in target/release, pytest files of the C++ DLL).
PORTED = {
    "xgameruntime.dll": (
        "xgameruntime.dll",
        [str(p.relative_to(REPO)) for pattern in ("tests/test_gdk_*.py", "tests/test_runtime_*.py")
         for p in sorted(REPO.glob(pattern))] + ["tests/test_native_services.py"],
    ),
}
pytestmark = pytest.mark.skipif(sys.platform != "win32", reason="Windows DLLs")
# Skip reasons that only mean the machine lacks an optional component.
ALLOWED_SKIPS = ("runtime GDK de Windows absent",)


def mixed_runtime(tmp_path, name, rust_file):
    folder = tmp_path / "runtime"
    shutil.copytree(RUNTIME_DIR, folder)
    shutil.copyfile(RUST_RELEASE / rust_file, folder / name)
    return folder


@pytest.mark.parametrize("name", sorted(PORTED))
def test_cpp_tests_pass_against_the_rust_dll(tmp_path, name):
    rust_file, test_files = PORTED[name]
    missing = [p for p in (RUST_RELEASE / rust_file,) if not p.is_file()]
    if missing or not runtime_built():
        if os.environ.get("ANYXBOXX_REQUIRE_RUNTIME") == "1":
            pytest.fail(f"absent: {missing or RUNTIME_DIR} (cargo build --release; cmake runtime/)")
        pytest.skip("Rust DLL or C++ runtime not built")
    folder = mixed_runtime(tmp_path, name, rust_file)
    env = {**os.environ, "ANYXBOXX_RUNTIME": str(folder), "ANYXBOXX_REQUIRE_RUNTIME": "1",
           "ANYXBOXX_TEST_RUST_XGAMERUNTIME": "1"}
    done = subprocess.run([sys.executable, "-m", "pytest", "-q", "-rs", "-p", "no:cacheprovider",
                           f"--basetemp={tmp_path / 'nested'}", *test_files],
                          cwd=REPO, env=env, capture_output=True, text=True, timeout=600)
    summary = done.stdout.strip().splitlines()[-1] if done.stdout.strip() else done.stderr
    assert done.returncode == 0, done.stdout[-4000:]
    passed = re.search(r"(\d+) passed", summary)
    skips = [line for line in done.stdout.splitlines() if line.startswith("SKIPPED")]
    # Only skips that depend on the machine are accepted: the Windows GDK runtime is not
    # installed on CI, and that test compares against it, not against our DLL.
    unexpected = [line for line in skips if not any(reason in line for reason in ALLOWED_SKIPS)]
    assert passed and int(passed.group(1)) > 0 and not unexpected, (summary, unexpected)


def exports(path):
    image = pefile.PE(str(path), fast_load=True)
    image.parse_data_directories(directories=[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_EXPORT"]])
    directory = getattr(image, "DIRECTORY_ENTRY_EXPORT", None)
    return {(s.name, s.forwarder) for s in directory.symbols} if directory else set()


@pytest.mark.parametrize("name", sorted(PORTED))
def test_rust_dll_exports_what_the_cpp_dll_exports(name):
    rust = RUST_RELEASE / PORTED[name][0]
    if not rust.is_file() or not (RUNTIME_DIR / name).is_file():
        if os.environ.get("ANYXBOXX_REQUIRE_RUNTIME") == "1":
            pytest.fail(f"absent: {rust} or {RUNTIME_DIR / name}")
        pytest.skip("Rust DLL or C++ runtime not built")
    assert exports(rust) == exports(RUNTIME_DIR / name)
