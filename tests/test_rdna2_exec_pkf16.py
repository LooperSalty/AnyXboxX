"""Tests of packed f16, v_pk_fmac_f16, v_fma_mix_*, and v_dot2_f32_f16 in the RDNA 2 reference interpreter."""
import pytest

from emu import rdna2_exec, rdna2_exec_f16, rdna2_exec_pkf16
from emu.rdna2_exec_ops import bits_float, float_bits
from emu.rdna2_ops import MNEMONICS
from test_rdna2_exec import OPCODES, V, const, program, vop1, vop2, vop3

LITERAL = 255
HALF_ONE = 0x3C00
HALF_TWO = 0x4000
HALF_NEG_HALF = 0xB800
HALF_NAN = 0x7E00


def pack_f16(lo, hi):
    return rdna2_exec_f16.half_bits(lo) | (rdna2_exec_f16.half_bits(hi) << 16)


def unpack_f16(word):
    return (rdna2_exec_f16.bits_half(word & 0xFFFF), rdna2_exec_f16.bits_half((word >> 16) & 0xFFFF))


def vop3p(name, vdst, src0, src1, src2=0, *, op_sel=0, op_sel_hi=0b111, neg_lo=0, neg_hi=0, clamp=0):
    opcode = OPCODES["VOP3P", name]
    w0 = (
        0x33 << 26
        | (opcode & 0x7F) << 16
        | (clamp & 1) << 15
        | ((op_sel_hi >> 2) & 1) << 14
        | (op_sel & 0x7) << 11
        | (neg_hi & 0x7) << 8
        | (vdst & 0xFF)
    )
    w1 = (
        (neg_lo & 0x7) << 29
        | ((op_sel_hi >> 1) & 1) << 28
        | (op_sel_hi & 1) << 27
        | (src2 & 0x1FF) << 18
        | (src1 & 0x1FF) << 9
        | (src0 & 0x1FF)
    )
    return (w0, w1)


def test_vop3p_coverage_is_complete():
    missing = [name for (family, _), name in MNEMONICS.items()
               if family == "VOP3P" and not rdna2_exec.executable(family, name)]
    assert missing == []
    assert rdna2_exec.executable("VOP2", "v_pk_fmac_f16")
    assert rdna2_exec.executable("VOP3", "v_pk_fmac_f16")


def test_packed_f16_add_mul_fma_min_max_and_modifiers():
    a = pack_f16(1.5, -2.0)
    b = pack_f16(2.0, 3.0)
    c = pack_f16(0.25, -1.0)
    code = program(
        vop1("v_mov_b32", 1, LITERAL), a,
        vop1("v_mov_b32", 2, LITERAL), b,
        vop1("v_mov_b32", 3, LITERAL), c,
        *vop3p("v_pk_add_f16", 4, V + 1, V + 2),                            # (3.5, 1.0)
        *vop3p("v_pk_mul_f16", 5, V + 1, V + 2),                            # (3.0, -6.0)
        *vop3p("v_pk_fma_f16", 6, V + 1, V + 2, V + 3),                     # (3.25, -7.0)
        *vop3p("v_pk_min_f16", 7, V + 1, V + 2),                            # (1.5, -2.0)
        *vop3p("v_pk_max_f16", 8, V + 1, V + 2),                            # (2.0, 3.0)
        # op_sel swaps a's halves for lo, op_sel_hi broadcasts b.lo, neg_lo negates src0.lo, neg_hi negates src1.hi.
        *vop3p("v_pk_add_f16", 9, V + 1, V + 2, op_sel=0b001, op_sel_hi=0b001, neg_lo=0b001, neg_hi=0b010),
        # clamp to [0, 1]: lo=3.25 -> 1.0, hi=-7.0 -> +0.0.
        *vop3p("v_pk_fma_f16", 10, V + 1, V + 2, V + 3, clamp=1),
        # Inline float constant 1.0 (242) replicates to both f16 halves.
        *vop3p("v_pk_add_f16", 11, 242, V + 1),                             # (2.5, -1.0)
    )
    wave = rdna2_exec.run(code)
    assert unpack_f16(wave.v[4][0]) == (3.5, 1.0)
    assert unpack_f16(wave.v[5][0]) == (3.0, -6.0)
    assert unpack_f16(wave.v[6][0]) == (3.25, -7.0)
    assert unpack_f16(wave.v[7][0]) == (1.5, -2.0)
    assert unpack_f16(wave.v[8][0]) == (2.0, 3.0)
    # lo: -(-2.0) + 2.0 = 4.0 ; hi: -2.0 + -(2.0) = -4.0
    assert unpack_f16(wave.v[9][0]) == (4.0, -4.0)
    assert wave.v[10][0] == HALF_ONE                                        # lo=1.0, hi=+0.0
    assert unpack_f16(wave.v[11][0]) == (2.5, -1.0)


def test_packed_f16_min_max_nan_and_signed_zero():
    nan_and_neg0 = HALF_NAN | (0x8000 << 16)
    two_and_pos0 = HALF_TWO | (0x0000 << 16)
    code = program(
        vop1("v_mov_b32", 1, LITERAL), nan_and_neg0,
        vop1("v_mov_b32", 2, LITERAL), two_and_pos0,
        *vop3p("v_pk_min_f16", 3, V + 1, V + 2),
        *vop3p("v_pk_max_f16", 4, V + 1, V + 2),
    )
    wave = rdna2_exec.run(code)
    assert wave.v[3][0] == (HALF_TWO | (0x8000 << 16))                      # NaN yields other, -0 < +0
    assert wave.v[4][0] == (HALF_TWO | (0x0000 << 16))                      # +0 > -0


def test_v_pk_fmac_f16_vop2_and_vop3():
    code = program(
        vop1("v_mov_b32", 1, LITERAL), pack_f16(2.0, -3.0),
        vop1("v_mov_b32", 2, LITERAL), pack_f16(1.5, 4.0),
        vop1("v_mov_b32", 3, LITERAL), pack_f16(0.5, 2.0),
        vop1("v_mov_b32", 4, LITERAL), pack_f16(0.5, 2.0),
        vop1("v_mov_b32", 5, LITERAL), pack_f16(1.0, -1.0),
        vop2("v_pk_fmac_f16", 3, V + 1, 2),                                 # (3.5, -10.0)
        *vop3("v_pk_fmac_f16", 4, V + 1, V + 2, 0, clamp=1),                # clamped -> (1.0, +0.0)
        vop2("v_pk_fmac_f16", 5, 240, 2),                                   # 0.5 * (1.5, 4.0) + (1.0, -1.0) = (1.75, 1.0)
    )
    wave = rdna2_exec.run(code)
    assert unpack_f16(wave.v[3][0]) == (3.5, -10.0)
    assert wave.v[4][0] == HALF_ONE
    assert unpack_f16(wave.v[5][0]) == (1.75, 1.0)


def test_v_fma_mix_f32_mixlo_and_mixhi():
    # Single-rounding check in f32: (1 + 2^-23) * (1 - 2^-24) - 1 = 2^-24 - 2^-47 (0x337FFFFE), not 0.
    one_plus = 0x3F80_0001
    one_minus = 0x3F7F_FFFF
    neg_one = float_bits(-1.0)
    code = program(
        vop1("v_mov_b32", 1, LITERAL), pack_f16(-2.5, 3.0),
        vop1("v_mov_b32", 2, LITERAL), float_bits(4.0),
        vop1("v_mov_b32", 3, LITERAL), pack_f16(0.5, -1.5),
        vop1("v_mov_b32", 4, LITERAL), 0xBEEF_CAFE,
        vop1("v_mov_b32", 5, LITERAL), 0xBEEF_CAFE,
        vop1("v_mov_b32", 6, LITERAL), one_plus,
        vop1("v_mov_b32", 7, LITERAL), one_minus,
        vop1("v_mov_b32", 8, LITERAL), neg_one,
        # src0 = f16 lo with abs (|-2.5| = 2.5), src1 = f32 (4.0), src2 = f16 hi with neg (-(-1.5) = +1.5) -> 11.5
        *vop3p("v_fma_mix_f32", 9, V + 1, V + 2, V + 3, op_sel=0b100, op_sel_hi=0b101, neg_lo=0b100, neg_hi=0b001),
        # Same inputs clamped to [0, 1] -> 1.0.
        *vop3p("v_fma_mix_f32", 10, V + 1, V + 2, V + 3, op_sel=0b100, op_sel_hi=0b101, neg_lo=0b100, neg_hi=0b001,
               clamp=1),
        # mixlo writes f16(2.5 * 4.0 + 0.5 = 10.5) to lo, preserves 0xBEEF in hi.
        *vop3p("v_fma_mixlo_f16", 4, V + 1, V + 2, V + 3, op_sel=0b000, op_sel_hi=0b101, neg_lo=0b000, neg_hi=0b001),
        # mixhi writes f16(-2.5 * 4.0 + 0.5 = -9.5) clamped to +0.0 into hi, preserves 0xCAFE in lo.
        *vop3p("v_fma_mixhi_f16", 5, V + 1, V + 2, V + 3, op_sel=0b000, op_sel_hi=0b101, neg_lo=0b000, neg_hi=0b000,
               clamp=1),
        # Pure f32 single-rounding FMA check (op_sel_hi = 0).
        *vop3p("v_fma_mix_f32", 11, V + 6, V + 7, V + 8, op_sel=0b000, op_sel_hi=0b000),
    )
    wave = rdna2_exec.run(code)
    assert bits_float(wave.v[9][0]) == 11.5
    assert wave.v[10][0] == float_bits(1.0)
    assert wave.v[4][0] == (0xBEEF_0000 | rdna2_exec_f16.half_bits(10.5))
    assert wave.v[5][0] == 0x0000_CAFE
    assert wave.v[11][0] == 0x337F_FFFE


def test_v_dot2_f32_f16():
    code = program(
        vop1("v_mov_b32", 1, LITERAL), pack_f16(1.5, -2.0),
        vop1("v_mov_b32", 2, LITERAL), pack_f16(4.0, 3.0),
        vop1("v_mov_b32", 3, LITERAL), float_bits(10.0),
        # 1.5*4.0 + (-2.0)*3.0 + 10.0 = 6.0 - 6.0 + 10.0 = 10.0
        *vop3p("v_dot2_f32_f16", 4, V + 1, V + 2, V + 3),
        # neg_lo[2] negates src2, neg_hi[0] negates a.hi -> 6.0 + 6.0 - 10.0 = 2.0, clamped -> 1.0
        *vop3p("v_dot2_f32_f16", 5, V + 1, V + 2, V + 3, neg_lo=0b100, neg_hi=0b001, clamp=1),
        # Inline float 2.0 (244) replicated on src0: 2*4 + 2*3 + 10 = 24.0
        *vop3p("v_dot2_f32_f16", 6, 244, V + 2, V + 3),
    )
    wave = rdna2_exec.run(code)
    assert bits_float(wave.v[4][0]) == 10.0
    assert wave.v[5][0] == float_bits(1.0)
    assert bits_float(wave.v[6][0]) == 24.0


def test_pkf16_refuses_undefined_modifiers():
    # op_sel on an f32 source of v_fma_mix_f32.
    bad_mix = program(*vop3p("v_fma_mix_f32", 4, V + 1, V + 2, V + 3, op_sel=0b001, op_sel_hi=0b000))
    with pytest.raises(rdna2_exec.Untranslated, match="op_sel"):
        rdna2_exec.run(bad_mix)

    # op_sel[2] on 32-bit f32 accumulator of v_dot2_f32_f16.
    bad_dot = program(*vop3p("v_dot2_f32_f16", 4, V + 1, V + 2, V + 3, op_sel=0b100))
    with pytest.raises(rdna2_exec.Untranslated, match="src2"):
        rdna2_exec.run(bad_dot)

    # VOP3 v_pk_fmac_f16 with unsupported abs/omod.
    bad_fmac = program(*vop3("v_pk_fmac_f16", 4, V + 1, V + 2, 0, abs_=0b001))
    with pytest.raises(rdna2_exec.Untranslated, match="abs"):
        rdna2_exec.run(bad_fmac)

    # Helper check directly for clamp_f32 on NaN and -0.0.
    assert rdna2_exec_pkf16.clamp_f32(0x7FC0_0000) == 0
    assert rdna2_exec_pkf16.clamp_f32(0x8000_0000) == 0
    assert rdna2_exec_pkf16.clamp_f32(float_bits(2.5)) == float_bits(1.0)
    assert rdna2_exec_pkf16.clamp_f32(float_bits(0.5)) == float_bits(0.5)
    assert const(0) == 128
