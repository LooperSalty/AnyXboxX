"""v_bfe_i32 on a field that runs past bit 31, in the RDNA 2 interpreter and the HLSL recompiler.

ISA RDNA 2 (section 12.12, p. 169): "D.i = (S0.i >> S1.u[4:0]) & ((1 << S2.u[4:0]) - 1)", a shift of the
signed S0, like s_bfe_i32 (section 12.1, p. 105), then the field is sign-extended. Before the fix, both
shifted S0 as unsigned: with offset + width > 32 the bits above bit 31 were zeros instead of copies of the
sign bit (0x939108F5, offset 16, width 31 gave 0x00009391 instead of 0xFFFF9391). Expected values agree
with LLVM's constant folding of llvm.amdgcn.sbfe (opt -passes=instcombine, gfx1030), which turns such a
field into an arithmetic shift of S0 by the offset.
"""
import pytest

from emu import rdna2_exec, rdna2_gpu
from test_rdna2_exec import V, const, program, vop1, vop3
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

LITERAL = 255
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")

CASES = [
    # S0, offset, width, expected
    (0xD992_A382, 31, 31, 0xFFFF_FFFF), (0x9391_08F5, 16, 31, 0xFFFF_9391), (0x1391_08F5, 16, 31, 0x0000_1391),
    (0x8000_0000, 28, 8, 0xFFFF_FFF8), (0xEC3B_1D4F, 16, 24, 0xFFFF_EC3B), (0xEC3B_1D4F, 48, 56, 0xFFFF_EC3B),
    # offset + width <= 32: the field is inside S0, unchanged by the fix
    (0x0000_00EE, 4, 4, 0xFFFF_FFFE), (0xF000_0000, 28, 4, 0xFFFF_FFFF), (0x7000_0000, 28, 4, 7),
    (0x9391_08F5, 16, 0, 0), (0x9391_08F5, 0, 31, 0x1391_08F5),
]


def test_bfe_i32_sign_extends_past_bit_31():
    results = {}
    for value, offset, width, _ in CASES:
        bfe = vop3("v_bfe_i32", 2, V + 1, const(offset), const(width))
        wave = rdna2_exec.run(program(vop1("v_mov_b32", 1, LITERAL), value, *bfe))
        results[value, offset, width] = f"{wave.v[2][0]:08X}"
    assert results == {(value, offset, width): f"{expected:08X}" for value, offset, width, expected in CASES}


def bfe_program():
    """Offsets and widths 0-31 taken from the lane number (v0), on a negative and a positive S0."""
    words = []
    for index, value in enumerate((0x9391_08F5, 0x6C3B_1D4F)):
        words += [vop1("v_mov_b32", 1, LITERAL), value,
                  *vop3("v_bfe_i32", 2, V + 1, V + 0, const(31)),      # width 31, offset = lane
                  *vop3("v_bfe_i32", 3, V + 1, const(16), V + 0),      # offset 16, width = lane
                  *vop3("v_bfe_i32", 4, V + 1, V + 0, V + 0)]          # offset = width = lane
        for slot, register in enumerate((2, 3, 4)):
            words += store_lane(register, 3 * index + slot)
    return with_lane_offset(*words)


def test_bfe_i32_lanes_follow_the_arithmetic_shift():
    wave = rdna2_exec.run(program(vop1("v_mov_b32", 1, LITERAL), 0x9391_08F5,
                                  *vop3("v_bfe_i32", 2, V + 1, V + 0, const(31))))
    signed_value = 0x9391_08F5 - (1 << 32)
    # lane 0: bits 31 and 30 of S0 differ, so the 31-bit field 0x139108F5 is positive
    assert wave.v[2] == [(signed_value >> lane) & 0xFFFF_FFFF if lane else 0x1391_08F5 for lane in range(32)]


@gpu
def test_bfe_i32_matches_interpreter_on_gpu():
    expected, actual = compare(bfe_program())
    assert actual == expected
