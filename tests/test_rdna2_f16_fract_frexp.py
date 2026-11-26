"""v_fract_f16, v_frexp_mant_f16 and v_frexp_exp_i16_f16 (VOP1 and VOP3) in the reference interpreter.

AMD "RDNA 2" ISA (November 2020), section 12.8, VOP1 opcodes 89, 90 and 95 (p. 147-148). They follow the
interpreter's f32 forms: fract = S0 + -floor(S0) at most the largest f16 below 1.0 (0x3BFF), NaN for ±inf;
frexp splits S0 = m × 2^e with 0.5 ≤ |m| < 1, ±0, ±inf and NaN giving S0 and 0. A denormal is normalised
(LLVM's constant folding does the same). Expected values are written by hand.
"""
import pytest

from emu import rdna2_exec, rdna2_hlsl
from test_rdna2_exec import ONE, V, program, vop1, vop3

OLD = 0x1234_0000               # destination's high half, which a low-half write keeps
# S0 → (fract, frexp_mant, frexp_exp as i16)
CASES = {
    0x3E00: (0x3800, 0x3A00, 1),          # 1.5 → 0.5 ; 0.75 × 2^1
    0xBCCD: (0x3A66, 0xB8CD, 1),          # -1.2 → 0.8 (DX fract) ; -0.6 × 2^1
    0xC500: (0x0000, 0xB900, 3),          # -5.0 → 0 ; -0.625 × 2^3
    0x3C00: (0x0000, 0x3800, 1),          # 1.0
    0x7BFF: (0x0000, 0x3BFF, 16),         # 65504
    0x0001: (0x0001, 0x3800, -23),        # smallest denormal: 0.5 × 2^-23
    0x8001: (0x3BFF, 0xB800, -23),        # 1 - 2^-24 rounds to 1.0: bounded to 0x3BFF
    0x03FF: (0x03FF, 0x3BFE, -14),        # largest denormal
    0x0000: (0x0000, 0x0000, 0),
    0x8000: (0x0000, 0x8000, 0),          # fract(-0) = +0, as v_fract_f32
    0x7C00: (0x7E00, 0x7C00, 0),
    0xFC00: (0x7E00, 0xFC00, 0),
    0x7D00: (0x7E00, 0x7D00, 0),          # NaN: frexp_mant gives S0 unchanged
}
INPUTS = list(CASES)
NAMES = ("v_fract_f16", "v_frexp_mant_f16", "v_frexp_exp_i16_f16")


def run(*words):
    wave = rdna2_exec.Wave()
    wave.v[1] = [OLD | INPUTS[lane % len(INPUTS)] for lane in range(32)]
    wave.v[2] = [INPUTS[lane % len(INPUTS)] << 16 | 0xABCD for lane in range(32)]
    for register in range(3, 8):
        wave.v[register] = [OLD | 0x5555] * 32
    rdna2_exec.run(program(*words), wave)
    return wave


def want(index, lane):
    return CASES[INPUTS[lane % len(INPUTS)]][index] & 0xFFFF


@pytest.mark.parametrize("index, name", enumerate(NAMES))
def test_vop1_writes_the_low_half(index, name):
    wave = run(vop1(name, 3, V + 1))
    assert wave.v[3] == [OLD | want(index, lane) for lane in range(32)]
    assert rdna2_exec.executable("VOP1", name) and rdna2_exec.executable("VOP3", name)


@pytest.mark.parametrize("index, name", enumerate(NAMES))
def test_vop3_op_sel_reads_and_writes_the_high_half(index, name):
    wave = run(*vop3(name, 3, V + 2, op_sel=0b1001))
    assert wave.v[3] == [want(index, lane) << 16 | 0x5555 for lane in range(32)]


def test_vop3_neg_and_inline_constant():
    wave = run(*vop3("v_fract_f16", 3, V + 1, neg=0b001),         # fract(-1.5) = 0.5
               vop1("v_frexp_exp_i16_f16", 4, ONE))              # 1.0 = 0.5 × 2^1
    assert wave.v[3][INPUTS.index(0x3E00)] == OLD | 0x3800
    assert wave.v[4][0] == OLD | 1


def test_clamp_on_the_integer_exponent_is_refused():
    with pytest.raises(rdna2_exec.Untranslated, match="clamp"):
        run(*vop3("v_frexp_exp_i16_f16", 3, V + 1, clamp=1))


@pytest.mark.parametrize("name", NAMES)
def test_hlsl_still_refuses_them(name):
    with pytest.raises(rdna2_hlsl.Untranslatable):
        rdna2_hlsl.translate(program(vop1(name, 3, V + 1)))
