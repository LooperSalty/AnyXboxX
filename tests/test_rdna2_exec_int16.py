"""16-bit integer VALU (VOP3 16-bit ops, v_mad_u32_u16/v_mad_i32_i16, v_cvt_pk_*16_*32, v_add/sub_nc_i32, packed
VOP3P integer ops): interpreter semantics, refused forms and HLSL refusals. GPU parity: test_rdna2_hlsl_int16.py.

Expected values are worked out by hand in the comments, from the AMD "RDNA 2" ISA pseudocode (November 2020,
sections 12.10 and 12.12), section 6.5 (clamp) and section 13.3 (OPSEL), cross-checked with LLVM 20.1.8 where
the comment says so (sources listed in emu/rdna2_exec_int16.py). None of these values is a hardware measurement.
"""
import pytest

from emu import rdna2_exec, rdna2_exec_int16, rdna2_hlsl
from test_rdna2_exec import OPCODES, V, const, program, vop1, vop3

LITERAL, ONE, MINUS_SIXTEEN = 255, 242, 208
OLD = 0xA5A5_A5A5                               # destination preset: shows which half was written
VOP3P_PREFIX = 0x33 << 26


def vop3p(name, vdst, src0, src1, src2=128, op_sel=0, op_sel_hi=0b111, neg_lo=0, neg_hi=0, clamp=0):
    low = (VOP3P_PREFIX | OPCODES["VOP3P", name] << 16 | clamp << 15 | (op_sel_hi >> 2 & 1) << 14 | op_sel << 11
           | neg_hi << 8 | vdst)
    high = neg_lo << 29 | (op_sel_hi & 0b11) << 27 | src2 << 18 | src1 << 9 | src0
    return low, high


def mov(vdst, value):
    return vop1("v_mov_b32", vdst, LITERAL), value & 0xFFFF_FFFF


def run_vop3(name, a, b, c=0, op_sel=0, clamp=0, sources=(V + 1, V + 2, V + 3), literal=()):
    wave = rdna2_exec.run(program(*mov(1, a), *mov(2, b), *mov(3, c), *mov(4, OLD),
                                  *vop3(name, 4, *sources, op_sel=op_sel, clamp=clamp), *literal))
    return wave.v[4][0]


def run_packed(name, a, b, c=0, literal=(), sources=(V + 1, V + 2, V + 3), **modifiers):
    wave = rdna2_exec.run(program(*mov(1, a), *mov(2, b), *mov(3, c), *mov(4, OLD),
                                  *vop3p(name, 4, *sources, **modifiers), *literal))
    return wave.v[4][0]


@pytest.mark.parametrize("name, a, b, c, op_sel, clamp, expected", [
    ("v_add_nc_i16", 0x7FFF, 1, 0, 0, 0, 0xA5A5_8000),                  # wraps; D[31:16] kept (LLVM gfx10)
    ("v_add_nc_i16", 0x7FFF, 1, 0, 0, 1, 0xA5A5_7FFF),                  # saturates to 32767 (LLVM saddsat)
    ("v_sub_nc_i16", 0x8000, 1, 0, 0, 1, 0xA5A5_8000),                  # -32768 - 1 → -32768 (LLVM ssubsat)
    ("v_sub_nc_i16", 0x8000, 1, 0, 0, 0, 0xA5A5_7FFF),
    ("v_add_nc_i16", 0x0001_0005, 0x7FFE, 0, 0b1001, 0, 0x7FFF_A5A5),   # op_sel: S0[31:16] = 1, D[31:16]
    ("v_add_nc_u16", 0xFFFF, 2, 0, 0, 0, 0xA5A5_0001),
    ("v_add_nc_u16", 0xFFFF, 2, 0, 0, 1, 0xA5A5_FFFF),
    ("v_add_nc_u16", 0x0003_0000, 4, 0, 0b1001, 0, 0x0007_A5A5),
    ("v_sub_nc_u16", 1, 2, 0, 0, 0, 0xA5A5_FFFF),
    ("v_sub_nc_u16", 1, 2, 0, 0, 1, 0xA5A5_0000),                       # unsigned floor
    ("v_mul_lo_u16", 0x1234, 0xAAAA, 0, 0, 0, 0xA5A5_9E88),             # 0x0C229E88: low half 0x9E88
    ("v_mul_lo_u16", 0xFFFF_0000, 0x0002_0000, 0, 0b0011, 0, 0xA5A5_FFFE),
    ("v_lshlrev_b16", 17, 0x8001, 0, 0, 0, 0xA5A5_0002),                # count 17 & 15 = 1
    ("v_lshrrev_b16", 4, 0xFFFF_8000, 0, 0, 0, 0xA5A5_0800),            # S1[31:16] ignored
    ("v_ashrrev_i16", 4, 0x8000, 0, 0, 0, 0xA5A5_F800),
    ("v_ashrrev_i16", 31, 0x8000, 0, 0, 0, 0xA5A5_FFFF),                # 31 & 15 = 15
    ("v_ashrrev_i16", 0x0002_0000, 0x4000_0000, 0, 0b1011, 0, 0x1000_A5A5),
    ("v_max_i16", 0x8000, 0x7FFF, 0, 0, 0, 0xA5A5_7FFF),
    ("v_max_u16", 0x8000, 0x7FFF, 0, 0, 0, 0xA5A5_8000),
    ("v_min_i16", 0x8000, 0x7FFF, 0, 0, 0, 0xA5A5_8000),
    ("v_min_u16", 0x8000, 0x7FFF, 0, 0, 0, 0xA5A5_7FFF),
    ("v_mad_u16", 0xFFFF, 0xFFFF, 0xFFFF, 0, 0, 0xA5A5_0000),          # 0xFFFE0001 + 0xFFFF = 0xFFFF0000
    ("v_mad_u16", 0xFFFF, 0xFFFF, 0xFFFF, 0, 1, 0xA5A5_FFFF),
    ("v_mad_i16", 0xFFFE, 0x4000, 0xFFFF, 0, 0, 0xA5A5_7FFF),          # -2 × 16384 - 1 = -32769 wraps
    ("v_mad_i16", 0xFFFE, 0x4000, 0xFFFF, 0, 1, 0xA5A5_8000),
    ("v_mad_i16", 0x0003_0000, 0x0005_0000, 0x0001_0000, 0b1111, 0, 0x0010_A5A5),   # 3 × 5 + 1 = 16
    ("v_min3_i16", 0x8000, 0xFFFF, 0x0001, 0, 0, 0xA5A5_8000),
    ("v_min3_u16", 0x8000, 0xFFFF, 0x0001, 0, 0, 0xA5A5_0001),
    ("v_max3_i16", 0x8000, 0xFFFF, 0x0001, 0, 0, 0xA5A5_0001),
    ("v_max3_u16", 0x8000, 0xFFFF, 0x0001, 0, 0, 0xA5A5_FFFF),
    ("v_med3_i16", 0x8000, 0xFFFF, 0x0001, 0, 0, 0xA5A5_FFFF),          # -32768, -1, 1 → -1
    ("v_med3_u16", 0x8000, 0xFFFF, 0x0001, 0, 0, 0xA5A5_8000),
    ("v_med3_u16", 0x0009_0000, 0x0002_0000, 0x0005_0000, 0b0111, 0, 0xA5A5_0005),
])
def test_16_bit_vop3_ops_write_one_half(name, a, b, c, op_sel, clamp, expected):
    assert run_vop3(name, a, b, c, op_sel, clamp) == expected


@pytest.mark.parametrize("name, a, b, c, op_sel, clamp, expected", [
    ("v_mad_u32_u16", 0xFFFF, 0xFFFF, 0xFFFF_FFFF, 0, 0, 0xFFFE_0000),        # 0x1_FFFE_0000 wraps
    ("v_mad_u32_u16", 0xFFFF, 0xFFFF, 0xFFFF_FFFF, 0, 1, 0xFFFF_FFFF),
    ("v_mad_u32_u16", 0x0002_0000, 0x0000_0003, 0x0000_0010, 0b0001, 0, 0x0000_0016),   # 2 × 3 + 0x10
    ("v_mad_i32_i16", 0x0003_FFFF, 0x0005_0000, 0, 0b0011, 0, 0x0000_000F),   # S0[31:16] × S1[31:16] = 3 × 5
    ("v_mad_i32_i16", 0x8000, 0x8000, 0x7FFF_FFFF, 0, 0, 0xBFFF_FFFF),   # 2^30 + 2^31 - 1 wraps
    ("v_mad_i32_i16", 0x8000, 0x8000, 0x7FFF_FFFF, 0, 1, 0x7FFF_FFFF),
    ("v_mad_i32_i16", 0xFFFF, 0x0002, 0x8000_0000, 0, 1, 0x8000_0000),   # -2 + INT32_MIN saturates
    ("v_cvt_pk_u16_u32", 0x0001_0000, 0x1234, 0, 0, 0, 0x1234_FFFF),
    ("v_cvt_pk_i16_i32", 0xFFFF_0000, 0x8000, 0, 0, 0, 0x7FFF_8000),     # -65536 → -32768, 32768 → 32767
    ("v_cvt_pk_i16_i32", 0xFFFF_FFFB, 7, 0, 0, 0, 0x0007_FFFB),
    ("v_add_nc_i32", 0x7FFF_FFFF, 1, 0, 0, 0, 0x8000_0000),
    ("v_add_nc_i32", 0x7FFF_FFFF, 1, 0, 0, 1, 0x7FFF_FFFF),
    ("v_add_nc_i32", 0x8000_0000, 0xFFFF_FFFF, 0, 0, 1, 0x8000_0000),
    ("v_sub_nc_i32", 0, 0x8000_0000, 0, 0, 0, 0x8000_0000),
    ("v_sub_nc_i32", 0, 0x8000_0000, 0, 0, 1, 0x7FFF_FFFF),
    ("v_sub_nc_i32", 0x8000_0000, 1, 0, 0, 1, 0x8000_0000),
    ("v_sub_nc_i32", 5, 7, 0, 0, 1, 0xFFFF_FFFE),
])
def test_32_bit_results(name, a, b, c, op_sel, clamp, expected):
    assert run_vop3(name, a, b, c, op_sel, clamp) == expected


def test_constants_on_16_bit_sources_are_their_32_bit_value_split():
    # LLVM 20.1.8 AMDGPUBaseInfo.cpp getInlineEncodingV216: integer inline constants are sign-extended 32-bit
    # values, float ones on UI16 instructions their float32 value. The literal under op_sel is assumed (#84).
    assert run_vop3("v_mad_u32_u16", 0, 0, sources=(V + 1, V + 2, ONE)) == 0x3F80_0000
    assert run_vop3("v_add_nc_u16", 0, 0, op_sel=0b0001, sources=(ONE, V + 2)) == 0xA5A5_3F80
    assert run_vop3("v_add_nc_u16", 0, 0, sources=(MINUS_SIXTEEN, V + 2)) == 0xA5A5_FFF0
    assert run_vop3("v_add_nc_u16", 0, 0, op_sel=0b0001, sources=(MINUS_SIXTEEN, V + 2)) == 0xA5A5_FFFF
    assert run_vop3("v_max_i16", 0, 0, op_sel=0b0001, sources=(LITERAL, V + 2), literal=(0x8001_7FFF,)) == 0xA5A5_0000


@pytest.mark.parametrize("name, a, b, c, modifiers, expected", [
    ("v_pk_add_u16", 0xFFFF_0001, 0x0001_FFFF, 0, {}, 0x0000_0000),
    ("v_pk_add_u16", 0xFFFF_0001, 0x0001_FFFF, 0, {"clamp": 1}, 0xFFFF_FFFF),
    ("v_pk_sub_u16", 0x0001_0005, 0x0002_0003, 0, {}, 0xFFFF_0002),
    ("v_pk_sub_u16", 0x0001_0005, 0x0002_0003, 0, {"clamp": 1}, 0x0000_0002),
    ("v_pk_add_i16", 0x7FFF_8000, 0x0001_FFFF, 0, {}, 0x8000_7FFF),
    ("v_pk_add_i16", 0x7FFF_8000, 0x0001_FFFF, 0, {"clamp": 1}, 0x7FFF_8000),
    ("v_pk_sub_i16", 0x8000_7FFF, 0x0001_FFFF, 0, {}, 0x7FFF_8000),
    ("v_pk_sub_i16", 0x8000_7FFF, 0x0001_FFFF, 0, {"clamp": 1}, 0x8000_7FFF),
    ("v_pk_mul_lo_u16", 0x1234_1234, 0xAAAA_0002, 0, {}, 0x9E88_2468),
    ("v_pk_max_i16", 0x8000_7FFF, 0x7FFF_8000, 0, {"clamp": 1}, 0x7FFF_7FFF),   # clamp no-op (section 6.5)
    ("v_pk_mad_u16", 0xFFFF_0002, 0xFFFF_0003, 0xFFFF_0004, {}, 0x0000_000A),
    ("v_pk_mad_u16", 0xFFFF_0002, 0xFFFF_0003, 0xFFFF_0004, {"clamp": 1}, 0xFFFF_000A),
    ("v_pk_mad_i16", 0x8000_FFFE, 0x8000_4000, 0x0000_FFFF, {}, 0x0000_7FFF),
    ("v_pk_mad_i16", 0x8000_FFFE, 0x8000_4000, 0x0000_FFFF, {"clamp": 1}, 0x7FFF_8000),
    ("v_pk_lshlrev_b16", 0x0011_0004, 0x8001_0001, 0, {}, 0x0002_0010),
    ("v_pk_lshrrev_b16", 0x000F_0001, 0x8000_8000, 0, {}, 0x0001_4000),
    ("v_pk_ashrrev_i16", 0x000F_0001, 0x8000_8000, 0, {}, 0xFFFF_C000),
    ("v_pk_max_i16", 0x8000_7FFF, 0x7FFF_8000, 0, {}, 0x7FFF_7FFF),
    ("v_pk_max_u16", 0x8000_7FFF, 0x7FFF_8000, 0, {"clamp": 1}, 0x8000_8000),
    ("v_pk_min_i16", 0x8000_7FFF, 0x7FFF_8000, 0, {}, 0x8000_8000),
    ("v_pk_min_u16", 0x8000_7FFF, 0x7FFF_8000, 0, {}, 0x7FFF_7FFF),
    # half 0 reads S0[31:16] + S1[15:0], half 1 reads S0[15:0] + S1[31:16]
    ("v_pk_add_u16", 0x0003_0001, 0x0010_0020, 0, {"op_sel": 0b001, "op_sel_hi": 0b010}, 0x0011_0023),
    ("v_pk_mad_u16", 0x0002_0003, 0x0004_0005, 0x0100_0200, {"op_sel": 0b111, "op_sel_hi": 0}, 0x020F_0108),
])
def test_packed_ops(name, a, b, c, modifiers, expected):
    assert run_packed(name, a, b, c, **modifiers) == expected


def test_packed_constants_split_like_registers():
    # -16 → 0xFFF0 / 0xFFFF, 1.0 → 0x0000 / 0x3F80, a literal whole: LLVM 20.1.8 getInlineEncodingV216, and
    # test/CodeGen/AMDGPU/add.v2i16.ll (GFX10) encodes <0, 16256> as 1.0 and <123, 456> as 0x1c8007b.
    assert run_packed("v_pk_add_u16", 0, 0, sources=(MINUS_SIXTEEN, V + 2)) == 0xFFFF_FFF0
    assert run_packed("v_pk_add_u16", 0, 0, sources=(ONE, V + 2)) == 0x3F80_0000
    assert run_packed("v_pk_add_u16", 0, 0, sources=(ONE, V + 2), op_sel=0b001, op_sel_hi=0b010) == 0x0000_3F80
    assert run_packed("v_pk_add_u16", 0, 0, sources=(LITERAL, V + 2), literal=(0x8001_7FFF,)) == 0x8001_7FFF
    assert run_packed("v_pk_max_i16", 0, 0, sources=(const(5), V + 2)) == 0x0000_0005


def test_two_source_packed_ops_ignore_the_third_selects():
    plain = run_packed("v_pk_sub_i16", 0x0005_0009, 0x0001_0002)
    assert plain == 0x0004_0007
    assert run_packed("v_pk_sub_i16", 0x0005_0009, 0x0001_0002, op_sel=0b100, op_sel_hi=0b011) == plain


def test_inactive_lanes_keep_their_destination():
    wave = rdna2_exec.Wave()
    wave.s[126] = 0x0000_FFFF
    wave.v[4] = [OLD] * 32
    rdna2_exec.run(program(*vop3("v_add_nc_u16", 4, V + 0, const(1)),
                           *vop3p("v_pk_add_u16", 5, V + 0, const(1))), wave)
    assert wave.v[4][3] == 0xA5A5_0004 and wave.v[4][16] == OLD
    assert wave.v[5][3] == 0x0000_0004 and wave.v[5][16] == 0


@pytest.mark.parametrize("words, name, message", [
    (vop3("v_add_nc_u16", 4, V + 1, V + 2, neg=0b001), "v_add_nc_u16", "neg"),
    (vop3("v_mad_u32_u16", 4, V + 1, V + 2, V + 3, abs_=0b100), "v_mad_u32_u16", "abs"),
    (vop3("v_add_nc_i32", 4, V + 1, V + 2, omod=1), "v_add_nc_i32", "omod"),
    (vop3("v_mul_lo_u16", 4, V + 1, V + 2, clamp=1), "v_mul_lo_u16", "clamp"),
    (vop3("v_mad_u32_u16", 4, V + 1, V + 2, V + 3, op_sel=0b0100), "v_mad_u32_u16", "op_sel"),
    (vop3("v_mad_i32_i16", 4, V + 1, V + 2, V + 3, op_sel=0b1000), "v_mad_i32_i16", "op_sel"),
    (vop3("v_lshlrev_b16", 4, V + 1, V + 2, clamp=1), "v_lshlrev_b16", "clamp"),
    (vop3("v_max_i16", 4, V + 1, V + 2, clamp=1), "v_max_i16", "clamp"),
    (vop3("v_med3_i16", 4, V + 1, V + 2, V + 3, clamp=1), "v_med3_i16", "clamp"),
    (vop3("v_cvt_pk_u16_u32", 4, V + 1, V + 2, clamp=1), "v_cvt_pk_u16_u32", "clamp"),
    (vop3("v_cvt_pk_i16_i32", 4, V + 1, V + 2, op_sel=0b0001), "v_cvt_pk_i16_i32", "op_sel"),
    (vop3("v_sub_nc_i32", 4, V + 1, V + 2, op_sel=0b1000), "v_sub_nc_i32", "op_sel"),
    (vop3p("v_pk_add_u16", 4, V + 1, V + 2, neg_lo=0b001), "v_pk_add_u16", "neg_lo"),
    (vop3p("v_pk_mad_i16", 4, V + 1, V + 2, V + 3, neg_hi=0b100), "v_pk_mad_i16", "neg_hi"),
    (vop3p("v_pk_mul_lo_u16", 4, V + 1, V + 2, clamp=1), "v_pk_mul_lo_u16", "clamp"),
    (vop3p("v_pk_lshlrev_b16", 4, V + 1, V + 2, clamp=1), "v_pk_lshlrev_b16", "clamp"),
    (vop3p("v_pk_ashrrev_i16", 4, V + 1, V + 2, clamp=1), "v_pk_ashrrev_i16", "clamp"),
])
def test_unverified_forms_are_refused_by_name(words, name, message):
    code = program(*words)
    with pytest.raises(rdna2_exec.Untranslated, match=f"(?=.*{name})(?=.*{message})"):
        rdna2_exec.run(code)
    with pytest.raises(rdna2_hlsl.Untranslatable, match=f"(?=.*{name})(?=.*{message})"):
        rdna2_hlsl.translate(code)


def test_the_family_counts_in_coverage():
    for name in rdna2_exec_int16.VOP3:
        assert rdna2_exec.executable("VOP3", name), name
    for name in rdna2_exec_int16.PACKED:
        assert rdna2_exec.executable("VOP3P", name), name
    assert len(rdna2_exec_int16.VOP3) + len(rdna2_exec_int16.PACKED) == 40
    assert set(rdna2_exec_int16.PACKED) | rdna2_exec_int16.VOP3 <= rdna2_hlsl.coverage_names()
