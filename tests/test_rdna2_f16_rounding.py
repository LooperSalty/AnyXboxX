"""v_floor_f16, v_ceil_f16, v_trunc_f16 and v_rndne_f16 (VOP1 and VOP3) in the reference interpreter.

AMD "RDNA 2" ISA (November 2020), section 12.8, VOP1 opcodes 91-94 (p. 148). Like the f32 forms of the
interpreter, the result keeps the sign of S0 (IEEE 754 roundToIntegral: trunc(-0.5) = -0), infinities are
unchanged and a NaN gives 0x7E00 (the rule of every f16 result, emu/rdna2_exec_f16.py). Expected values are
written by hand.
"""
import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import LITERAL, ONE, V, program, vop1, vop3
from test_rdna2_hlsl import compare, with_lane_offset
from test_rdna2_hlsl_f16 import LOADS, image, into_v10

gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc or anyxboxx_gpu_run.exe missing")
NAMES = ("v_floor_f16", "v_ceil_f16", "v_trunc_f16", "v_rndne_f16")
OLD = 0x1234_0000               # destination's high half, which a low-half write keeps
# S0 → (floor, ceil, trunc, rndne)
CASES = {
    0x3E00: (0x3C00, 0x4000, 0x3C00, 0x4000),      # 1.5
    0xBE00: (0xC000, 0xBC00, 0xBC00, 0xC000),      # -1.5
    0x4100: (0x4000, 0x4200, 0x4000, 0x4000),      # 2.5: ties to even
    0xC100: (0xC200, 0xC000, 0xC000, 0xC000),      # -2.5
    0xB800: (0xBC00, 0x8000, 0x8000, 0x8000),      # -0.5: -1, -0, -0, -0
    0x3800: (0x0000, 0x3C00, 0x0000, 0x0000),      # 0.5
    0x3A00: (0x0000, 0x3C00, 0x0000, 0x3C00),      # 0.75
    0x8000: (0x8000,) * 4,                         # -0
    0x0001: (0x0000, 0x3C00, 0x0000, 0x0000),      # smallest denormal
    0x8001: (0xBC00, 0x8000, 0x8000, 0x8000),
    0x6BFF: (0x6BFF,) * 4,                         # 4094: already whole
    0x7BFF: (0x7BFF,) * 4,                         # 65504
    0x7C00: (0x7C00,) * 4,
    0xFC00: (0xFC00,) * 4,
    0x7D00: (0x7E00,) * 4,                         # signalling NaN
    0xFE01: (0x7E00,) * 4,                         # negative NaN
}
INPUTS = list(CASES)


def run(*words):
    wave = rdna2_exec.Wave()
    wave.v[1] = [OLD | INPUTS[lane % len(INPUTS)] for lane in range(32)]
    wave.v[2] = [INPUTS[lane % len(INPUTS)] << 16 | 0xABCD for lane in range(32)]
    for register in range(3, 12):
        wave.v[register] = [OLD | 0x5555] * 32
    rdna2_exec.run(program(*words), wave)
    return wave


def want(index, lane):
    return CASES[INPUTS[lane % len(INPUTS)]][index]


@pytest.mark.parametrize("index, name", enumerate(NAMES))
def test_vop1_rounds_the_low_half_and_keeps_the_high_half(index, name):
    wave = run(vop1(name, 3, V + 1))
    assert wave.v[3] == [OLD | want(index, lane) for lane in range(32)]
    assert rdna2_exec.executable("VOP1", name) and rdna2_exec.executable("VOP3", name)


@pytest.mark.parametrize("index, name", enumerate(NAMES))
def test_vop3_op_sel_reads_and_writes_the_high_half(index, name):
    wave = run(*vop3(name, 3, V + 2, op_sel=0b1001))               # S0[31:16] → D[31:16]
    assert wave.v[3] == [want(index, lane) << 16 | 0x5555 for lane in range(32)]


def test_vop3_modifiers_and_clamp():
    wave = run(*vop3("v_floor_f16", 3, V + 1, neg=0b001),         # floor(-1.5) for S0 = 1.5 …
               *vop3("v_ceil_f16", 4, V + 1, abs_=0b001),         # ceil(|S0|)
               *vop3("v_trunc_f16", 5, V + 1, clamp=1))           # clamp to [0, 1]: NaN and -0 give +0
    lane = INPUTS.index(0x3E00)
    assert wave.v[3][lane] == OLD | 0xC000
    lane = INPUTS.index(0xBE00)
    assert wave.v[4][lane] == OLD | 0x4000
    for lane in range(len(INPUTS)):
        trunc = want(2, lane)
        clamped = 0 if trunc == 0x7E00 or trunc & 0x8000 else min(trunc, 0x3C00)
        assert wave.v[5][lane] == OLD | clamped


def test_constants_are_binary16():
    """Inline 1.0 is 0x3C00 for an f16 source; a literal gives its low 16 bits (0x4100 = 2.5)."""
    wave = run(vop1("v_floor_f16", 3, ONE), vop1("v_rndne_f16", 4, LITERAL), 0xDEAD_4100)
    assert (wave.v[3][0], wave.v[4][0]) == (OLD | 0x3C00, OLD | 0x4000)


@pytest.mark.parametrize("name, function", zip(NAMES[:3], ("floor", "ceil", "trunc")))
def test_hlsl_rounds_in_f32_and_returns_to_f16(name, function):
    hlsl = rdna2_hlsl.translate(program(vop1(name, 3, V + 1)))
    assert f"F32ToF16(asuint({function}(asfloat(F16ToF32((v[1] & 0xFFFFu))))))" in hlsl
    assert name in rdna2_hlsl.coverage_names()


def test_hlsl_rndne_is_integer_only():
    """No DXIL Round_ne: the NVIDIA 621.xx driver folds Round_ne(2.5) to 3.0 (#227)."""
    hlsl = rdna2_hlsl.translate(program(vop1("v_rndne_f16", 3, V + 1)))
    body = hlsl.split("[numthreads", 1)[1]
    assert "F16Rndne((v[1] & 0xFFFFu))" in body and "round(" not in body
    assert "v_rndne_f16" in rdna2_hlsl.coverage_names()


@gpu
def test_rounding_matches_the_interpreter_on_gpu():
    operations = [(vop1(name, 10, V + register),) for name in NAMES for register in (1, 2, 4)]
    operations += [vop3(name, 10, V + 2, op_sel=0b1001) for name in NAMES]
    operations += [vop3(name, 10, V + 1, neg=0b001) for name in NAMES]
    operations += [vop3(name, 10, V + 4, abs_=0b001, clamp=1) for name in NAMES]
    operations += [(vop1(name, 10, LITERAL), 0xDEAD_4100) for name in NAMES]
    expected, actual = compare(with_lane_offset(*LOADS, *into_v10(*operations)), memory=image())
    assert actual == expected
