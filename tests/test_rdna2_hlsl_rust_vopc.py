"""VOPC integer comparisons: text and WARP mask parity."""
import struct

import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import LITERAL, OPCODES, program, sop1, vopc
from test_rdna2_hlsl_rust_parity import rust

OPS = tuple(name for (family, name) in OPCODES if family == "VOPC"
            and name.endswith(("_u32", "_i32", "_u16", "_i16")))


@pytest.mark.parametrize("name", OPS)
@pytest.mark.parametrize("value,mask", [(0, 0xffffffff), (16, 0x55555555), (0xffff8000, 0xaaaaaaaa)])
def test_vopc_integer_text_and_warp(name, value, mask, monkeypatch):
    if not rdna2_gpu.available():
        pytest.skip("dxc or GPU runner is unavailable")
    monkeypatch.setenv("ANYXBOXX_GPU", "warp")
    data = program(sop1("s_mov_b32", 126, LITERAL), mask, sop1("s_mov_b32", 106, LITERAL), 0xaabbccdd,
                   vopc(name, LITERAL, 0), value)
    result = rust(data)
    assert result.returncode == 0, result.stderr.decode()
    hlsl = result.stdout.decode("utf-8")
    assert hlsl == rdna2_hlsl.translate(data)
    wave = rdna2_exec.run(data)
    hlsl = hlsl.replace("  return;\n", "  if (lane == 0) { g_memory.Store(0, s[106]); "
                        "g_memory.Store(4, s[126]); }\n  return;\n")
    assert rdna2_gpu.run(hlsl, bytes(8)) == struct.pack("<II", wave.s[106], wave.s[126])


@pytest.mark.parametrize("src", [0xe9, 0xea, 0xf9, 0xfa])
def test_unimplemented_vopc_extensions_are_named(src):
    result = rust(program(vopc("v_cmp_eq_u32", src, 0), 0))
    assert result.returncode != 0 and b"v_cmp_eq_u32" in result.stderr


def test_unimplemented_vopc_float_is_named():
    result = rust(program(vopc("v_cmp_eq_f32", 128, 0)))
    assert result.returncode != 0 and b"v_cmp_eq_f32" in result.stderr


@pytest.mark.parametrize("name", ["v_cmp_lt_i16", "v_cmp_gt_i16", "v_cmp_lt_u16", "v_cmp_gt_u16",
                                  "v_cmp_lt_i32", "v_cmp_gt_i32", "v_cmp_lt_u32", "v_cmp_gt_u32"])
def test_vector_source_and_negative_comparison(name, monkeypatch):
    if not rdna2_gpu.available():
        pytest.skip("dxc or GPU runner is unavailable")
    monkeypatch.setenv("ANYXBOXX_GPU", "warp")
    from test_rdna2_exec import V, vop1
    data = program(vop1("v_mov_b32", 1, LITERAL), 0xffff8000, vopc(name, V, 1))
    result = rust(data)
    assert result.returncode == 0, result.stderr.decode()
    hlsl = result.stdout.decode("utf-8")
    assert hlsl == rdna2_hlsl.translate(data)
    wave = rdna2_exec.run(data)
    hlsl = hlsl.replace("  return;\n", "  if (lane == 0) g_memory.Store(0, s[106]);\n  return;\n")
    assert rdna2_gpu.run(hlsl, bytes(4)) == struct.pack("<I", wave.s[106])
