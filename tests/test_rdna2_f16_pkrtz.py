"""v_cvt_pkrtz_f16_f32 (VOP2 and VOP3) in the reference interpreter: two f32 → a packed f16 pair, toward zero.

AMD "RDNA 2" ISA (November 2020), section 12.7, VOP2 opcode 47 (p. 135): "D.f16_lo = f32_to_f16(S0.f32);
D.f16_hi = f32_to_f16(S1.f32). Round-toward-zero regardless of current round mode setting". So a finite
value too large gives ±65504, never infinity. A NaN gives 0x7E00, the rule of every f16 result of
emu/rdna2_exec_f16.py. Expected values are written by hand.
"""
import struct

import pytest

from emu import rdna2_exec, rdna2_hlsl
from test_rdna2_exec import LITERAL, ONE, V, program, vop2, vop3

NAME = "v_cvt_pkrtz_f16_f32"


def f32(value):
    return struct.unpack("<I", struct.pack("<f", value))[0]


# f32 bits → binary16 toward zero (and, in the comment, what round-to-nearest would give)
CASES = [
    (f32(1.0), 0x3C00), (f32(2.0), 0x4000), (f32(-0.0), 0x8000),
    (f32(1 + 3 * 2**-11), 0x3C01),               # nearest: 0x3C02
    (f32(-(1 + 3 * 2**-11)), 0xBC01),
    (f32(65519.0), 0x7BFF), (f32(65520.0), 0x7BFF),  # nearest: 0x7C00 for 65520
    (f32(-1e9), 0xFBFF),
    (0x7F80_0000, 0x7C00), (0xFF80_0000, 0xFC00),
    (0x7FC0_0001, 0x7E00), (0x7F80_0001, 0x7E00), (0xFFFF_FFFF, 0x7E00),
    (f32(1.5 * 2**-24), 0x0001),                 # nearest: 0x0002 (tie to even)
    (f32(0.75 * 2**-24), 0x0000), (f32(-0.75 * 2**-24), 0x8000),
    (0x0000_0001, 0x0000), (0x8000_0001, 0x8000),  # f32 denormals
    (f32(2**-14 - 2**-30), 0x03FF),              # just under the smallest normal
]


def run(*words):
    wave = rdna2_exec.Wave()
    wave.v[1] = [CASES[lane % len(CASES)][0] for lane in range(32)]
    wave.v[2] = [CASES[(lane + 7) % len(CASES)][0] for lane in range(32)]
    rdna2_exec.run(program(*words), wave)
    return wave


def want(lane):
    return CASES[(lane + 7) % len(CASES)][1] << 16 | CASES[lane % len(CASES)][1]


def test_vop2_and_vop3_pack_toward_zero():
    wave = run(vop2(NAME, 3, V + 1, 2), *vop3(NAME, 4, V + 1, V + 2))
    assert wave.v[3] == wave.v[4] == [want(lane) for lane in range(32)]
    assert rdna2_exec.executable("VOP2", NAME) and rdna2_exec.executable("VOP3", NAME)


def test_vop3_abs_and_neg_act_on_the_f32_sign():
    wave = run(*vop3(NAME, 3, V + 1, V + 2, neg=0b001, abs_=0b010))
    assert (CASES[0][0], CASES[7][0]) == (f32(1.0), f32(-1e9))      # lane 0: S0 = 1.0, S1 = -1e9
    assert wave.v[3][0] == 0x7BFF_BC00                                 # -1.0 and |-1e9| toward zero


def test_constants_are_f32_sources():
    """Inline 1.0 is the f32 1.0; a literal is a whole f32 word."""
    wave = run(vop2(NAME, 3, ONE, 1), vop2(NAME, 4, LITERAL, 1), f32(65520.0))
    assert wave.v[3][0] == 0x3C00_3C00
    assert wave.v[4][0] == 0x3C00_7BFF


@pytest.mark.parametrize("fields, reason", [({"op_sel": 0b0001}, "op_sel"), ({"clamp": 1}, "clamp"),
                                            ({"omod": 1}, "omod")])
def test_unmeasured_vop3_forms_are_refused(fields, reason):
    with pytest.raises(rdna2_exec.Untranslated, match=reason):
        run(*vop3(NAME, 3, V + 1, V + 2, **fields))


def test_hlsl_still_refuses_it():
    with pytest.raises(rdna2_hlsl.Untranslatable):
        rdna2_hlsl.translate(program(vop2(NAME, 3, V + 1, 2)))
