"""Run synthetic native-launcher tests against both C++ and Rust executables."""
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from conftest import RUNTIME_DIR, runtime_built
from pe_builder import Asm, build_pe

REPO = Path(__file__).resolve().parents[1]
RUST = REPO / "target" / "release" / "anyxboxx-native-launcher.exe"
CPP = RUNTIME_DIR / "anyxboxx_cpp.exe"
pytestmark = pytest.mark.skipif(sys.platform != "win32", reason="Windows native launcher")


@pytest.mark.parametrize("rust", [False, True], ids=["cpp", "rust"])
def test_synthetic_native_launcher_suite(tmp_path, rust):
    if not runtime_built() or not CPP.is_file() or (rust and not RUST.is_file()):
        if os.environ.get("ANYXBOXX_REQUIRE_RUNTIME") == "1":
            pytest.fail("build the C++ runtime and cargo build --release before parity tests")
        pytest.skip("native runtime or Rust launcher not built")
    runtime = tmp_path / "runtime"
    shutil.copytree(RUNTIME_DIR, runtime)
    shutil.copyfile(RUST if rust else CPP, runtime / "anyxboxx.exe")
    env = {**os.environ, "ANYXBOXX_RUNTIME": str(runtime), "ANYXBOXX_REQUIRE_RUNTIME": "1",
           "PYTEST_DISABLE_PLUGIN_AUTOLOAD": "1"}
    result = subprocess.run([sys.executable, "-m", "pytest", "-q", "-p", "no:cacheprovider",
                             "--basetemp", str(tmp_path / "fixtures"), "tests/test_launcher.py"], cwd=REPO, env=env,
                            capture_output=True, text=True, timeout=600)
    assert result.returncode == 0, result.stdout[-6000:] + result.stderr[-2000:]
    match = re.search(r"(\d+) passed", result.stdout)
    assert match and int(match.group(1)) >= 8, result.stdout


def test_usage_and_invalid_pe_errors_match_cpp(tmp_path):
    if not runtime_built() or not CPP.is_file() or not RUST.is_file():
        if os.environ.get("ANYXBOXX_REQUIRE_RUNTIME") == "1":
            pytest.fail("build both native launchers and the runtime before parity tests")
        pytest.skip("native launchers or runtime not built")
    runtime = tmp_path / "runtime"
    shutil.copytree(RUNTIME_DIR, runtime)
    shutil.copyfile(RUST, runtime / "rust.exe")
    image = tmp_path / "synthetic-invalid.exe"
    image.write_bytes(b"not a PE")
    no_reloc = build_pe(tmp_path / "synthetic-no-reloc.exe", lambda iat: Asm().mov_eax(7).ret().build(),
                        dll=False, base=0xFFFF_0000_0000_0000)
    missing_import = build_pe(tmp_path / "synthetic-missing-import.exe", lambda iat: Asm().ret().build(),
                              imports=[("AnyXboxXHostAbsent.dll", 1)], dll=False)
    for args in ([], [str(image)], [str(no_reloc)], [str(missing_import)]):
        outputs = [subprocess.run([str(runtime / name), *args], capture_output=True, timeout=30)
                   for name in ("anyxboxx_cpp.exe", "rust.exe")]
        assert outputs[0].returncode == outputs[1].returncode == 1
        def decode(data):
            try:
                return data.decode("utf-8").replace("\r\n", "\n")
            except UnicodeDecodeError:
                return data.decode("mbcs").replace("\r\n", "\n")
        assert decode(outputs[0].stderr) == decode(outputs[1].stderr)
