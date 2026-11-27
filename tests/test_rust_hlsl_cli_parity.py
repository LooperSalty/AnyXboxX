"""Compare the user-facing RDNA 2 translation commands on synthetic shaders."""
import os
from pathlib import Path
import subprocess
import sys

import pytest

from test_rdna2_exec import const, program, sop1
from test_rdna2_hlsl_rust_parity import ROOT, TARGET


@pytest.mark.parametrize("options", [["--hlsl"], ["--spirv"], ["--spirv", "ps"]])
def test_shader_cli_output(options, tmp_path):
    binary = TARGET / "release" / ("anyxboxx.exe" if os.name == "nt" else "anyxboxx")
    if not binary.is_file():
        pytest.skip("build the anyxboxx CLI in release mode")
    shader = Path(tmp_path) / "test.bin"
    shader.write_bytes(program(sop1("s_mov_b32", 4, const(7))))
    env = {**os.environ, "PYTHONIOENCODING": "utf-8"}

    def invoke(prefix):
        return subprocess.run([*prefix, str(shader), *options], capture_output=True, env=env)

    python = invoke([sys.executable, ROOT / "PrototypeScarlett" / "scarlett.py", "--rdna2"])
    rust = invoke([binary, "rdna2"])
    assert rust.returncode == python.returncode == (0 if options == ["--hlsl"] else 2)
    assert rust.stdout == python.stdout
    assert rust.stderr.decode().splitlines() == python.stderr.decode().splitlines()
