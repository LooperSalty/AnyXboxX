"""Text and WARP parity for the Rust RDNA 2 HLSL recompiler."""
import os
from pathlib import Path
import subprocess

import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import const, program, sop1, sopp

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / "target" / "release" / ("anyxboxx-rdna2-hlsl.exe" if os.name == "nt" else "anyxboxx-rdna2-hlsl")


def rust(data):
    if not BINARY.is_file():
        if os.environ.get("ANYXBOXX_REQUIRE_RUNTIME") == "1":
            pytest.fail("build anyxboxx-rdna2-hlsl in release mode")
        pytest.skip("Rust HLSL recompiler is not built")
    return subprocess.run([str(BINARY)], input=data, capture_output=True, check=False)


@pytest.mark.parametrize("data", [program(), program(sopp("s_nop")), program(sopp("s_nop", 7))])
def test_text_matches_python(data):
    result = rust(data)
    assert result.returncode == 0, result.stderr.decode()
    assert result.stdout == rdna2_hlsl.translate(data).encode("utf-8")


def test_untranslated_is_named():
    result = rust(program(sop1("s_mov_b32", 0, const(1))))
    assert result.returncode != 0
    assert b"s_mov_b32" in result.stderr


def test_warp_matches_interpreter(monkeypatch):
    if not rdna2_gpu.available():
        pytest.skip("dxc or GPU runner is unavailable")
    monkeypatch.setenv("ANYXBOXX_GPU", "warp")
    data = program(sopp("s_nop"))
    result = rust(data)
    assert result.returncode == 0, result.stderr.decode()
    memory = bytes(256)
    wave = rdna2_exec.Wave()
    wave.memory.write(0, memory)
    rdna2_exec.run(data, wave)
    assert rdna2_gpu.run(result.stdout.decode("utf-8"), memory) == wave.memory.read(0, len(memory))
