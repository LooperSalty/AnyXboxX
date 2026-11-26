"""Text and WARP parity for the Rust RDNA 2 HLSL recompiler."""
import os
from pathlib import Path
import subprocess

import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import LITERAL, const, program, sop1, sopp

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
    result = rust(program(sop1("s_mov_b64", 0, const(1))))
    assert result.returncode != 0
    assert b"s_mov_b64" in result.stderr


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


SOP1_32 = (
    "s_mov_b32", "s_cmov_b32", "s_not_b32", "s_brev_b32", "s_bcnt1_i32_b32",
    "s_ff1_i32_b32", "s_flbit_i32_b32", "s_sext_i32_i8", "s_sext_i32_i16",
    "s_abs_i32", "s_bcnt0_i32_b32", "s_ff0_i32_b32", "s_flbit_i32",
    "s_wqm_b32", "s_quadmask_b32", "s_bitset0_b32", "s_bitset1_b32",
)


@pytest.mark.parametrize("name", SOP1_32)
@pytest.mark.parametrize("value", [0, 0x80000000, 0x12345678])
def test_sop1_32_text_and_warp(name, value, monkeypatch):
    if not rdna2_gpu.available():
        pytest.skip("dxc or GPU runner is unavailable")
    monkeypatch.setenv("ANYXBOXX_GPU", "warp")
    # Initialize the destination and SCC so read-modify-write and conditional moves are observable.
    data = program(sop1("s_mov_b32", 1, LITERAL), 0x87654321,
                   sop1("s_not_b32", 2, const(0)), sop1(name, 1, LITERAL), value)
    result = rust(data)
    assert result.returncode == 0, result.stderr.decode()
    hlsl = result.stdout.decode("utf-8")
    assert hlsl == rdna2_hlsl.translate(data)
    wave = rdna2_exec.run(data)
    # Instrument only the test shader: scalar state is otherwise invisible at s_endpgm.
    hlsl = hlsl.replace("  return;\n",
                        "  if (lane == 0) { g_memory.Store(0, s[1]); g_memory.Store(4, scc ? 1u : 0u); }\n  return;\n")
    import struct
    assert rdna2_gpu.run(hlsl, bytes(8)) == struct.pack("<II", wave.s[1], int(wave.scc))

@pytest.mark.parametrize("source", [0, 125, 128, 192, 193, 208, *range(240, 249)])
def test_sop1_source_encodings_match_warp(source, monkeypatch):
    if not rdna2_gpu.available():
        pytest.skip("dxc or GPU runner is unavailable")
    monkeypatch.setenv("ANYXBOXX_GPU", "warp")
    data = program(sop1("s_mov_b32", 1, source))
    result = rust(data)
    assert result.returncode == 0, result.stderr.decode()
    hlsl = result.stdout.decode("utf-8")
    assert hlsl == rdna2_hlsl.translate(data)
    wave = rdna2_exec.run(data)
    hlsl = hlsl.replace("  return;\n", "  if (lane == 0) g_memory.Store(0, s[1]);\n  return;\n")
    import struct
    assert rdna2_gpu.run(hlsl, bytes(4)) == struct.pack("<I", wave.s[1])


def test_sop1_rejects_unknown_operand():
    result = rust(program(sop1("s_mov_b32", 1, 209)))
    assert result.returncode != 0
    assert b"s_mov_b32" in result.stderr and b"209" in result.stderr

EXEC32 = tuple(f"s_{op}_saveexec_b32" for op in
               ("and", "or", "xor", "andn2", "orn2", "nand", "nor", "xnor", "andn1", "orn1")) + (
                   "s_andn1_wrexec_b32", "s_andn2_wrexec_b32")


@pytest.mark.parametrize("name", EXEC32)
@pytest.mark.parametrize("value", [0, 0x12345678])
def test_sop1_exec_text_and_warp(name, value, monkeypatch):
    if not rdna2_gpu.available():
        pytest.skip("dxc or GPU runner is unavailable")
    monkeypatch.setenv("ANYXBOXX_GPU", "warp")
    data = program(sop1(name, 1, LITERAL), value)
    result = rust(data)
    assert result.returncode == 0, result.stderr.decode()
    hlsl = result.stdout.decode("utf-8")
    assert hlsl == rdna2_hlsl.translate(data)
    wave = rdna2_exec.run(data)
    hlsl = hlsl.replace("  return;\n", "  if (lane == 0) { g_memory.Store(0, s[1]); "
                        "g_memory.Store(4, s[126]); g_memory.Store(8, scc ? 1u : 0u); }\n  return;\n")
    import struct
    assert rdna2_gpu.run(hlsl, bytes(12)) == struct.pack("<III", wave.s[1], wave.s[126], int(wave.scc))


def test_sop1_conditional_move_when_scc_is_clear(monkeypatch):
    if not rdna2_gpu.available():
        pytest.skip("dxc or GPU runner is unavailable")
    monkeypatch.setenv("ANYXBOXX_GPU", "warp")
    data = program(sop1("s_mov_b32", 1, const(7)), sop1("s_cmov_b32", 1, const(3)))
    result = rust(data)
    assert result.returncode == 0, result.stderr.decode()
    hlsl = result.stdout.decode("utf-8")
    assert hlsl == rdna2_hlsl.translate(data)
    wave = rdna2_exec.run(data)
    hlsl = hlsl.replace("  return;\n", "  if (lane == 0) g_memory.Store(0, s[1]);\n  return;\n")
    import struct
    assert rdna2_gpu.run(hlsl, bytes(4)) == struct.pack("<I", wave.s[1])
