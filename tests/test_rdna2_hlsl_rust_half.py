"""Rust VOP1/VOP3 half operations on per-lane edge values and partial EXEC."""
import struct

import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl, rdna2_hlsl_f16
from test_rdna2_exec import LITERAL, V, sop1, vop1, vop3
from test_rdna2_exec_memory import INPUT, OUTPUT
from test_rdna2_f16 import REFUSED_VOP3
from test_rdna2_f16_transcendentals import NAMES as TRANSCENDENTALS
from test_rdna2_hlsl import MEMORY_SIZE, with_lane_offset
from test_rdna2_hlsl_f16 import LOADS, image, into_v10
from test_rdna2_hlsl_f16conv import parity_memory, parity_program
from test_rdna2_hlsl_rust_parity import rust


def verify(data, memory):
    prefix = (sop1("s_mov_b32", 0, LITERAL), INPUT, sop1("s_mov_b32", 2, LITERAL), OUTPUT)
    data = struct.pack("<4I", *prefix) + data
    result = rust(data)
    assert result.returncode == 0, result.stderr.decode()
    hlsl = result.stdout.decode()
    assert hlsl == rdna2_hlsl.translate(data)
    if not rdna2_gpu.available():
        pytest.skip("dxc or GPU runner is unavailable")
    start = memory.ljust(MEMORY_SIZE, b"\0")
    wave = rdna2_exec.Wave()
    wave.memory.write(0, start)
    rdna2_exec.run(data, wave)
    assert rdna2_gpu.run(hlsl, start) == wave.memory.read(0, MEMORY_SIZE)


def test_conversions_packs_and_med3():
    data, _ = parity_program()
    verify(data, parity_memory())


def test_arithmetic_and_transcendentals():
    ops = [(vop1(name, 10, V + 1),) for name in TRANSCENDENTALS]
    for name in rdna2_hlsl_f16.TEMPLATES:
        if name in ("v_fmamk_f16", "v_fmaak_f16"):
            ops.append((*vop3(name, 10, LITERAL, V + 2), 0x3800))
        else:
            ops.append(vop3(name, 10, V + 1, V + 2, V + 4))
    ops.append(vop3("v_mul_f16", 10, V + 1, V + 2, abs_=1, neg=2, clamp=1, op_sel=0b1011))
    verify(with_lane_offset(*LOADS, *into_v10(*ops)), image())


@pytest.mark.parametrize("words,pattern", REFUSED_VOP3)
def test_refused_f16_modifiers(words, pattern):
    from test_rdna2_exec import program
    from emu import rdna2
    data = program(*words)
    inst = rdna2.decode(data)[0]
    name = rdna2.mnemonic(inst.family, inst.opcode)
    with pytest.raises(rdna2_hlsl.Untranslatable, match=pattern):
        rdna2_hlsl.translate(data)
    result = rust(data)
    assert result.returncode and name.encode() in result.stderr
