"""HLSL translation and GPU parity tests for packed f16, v_pk_fmac_f16, v_fma_mix_*, and v_dot2_f32_f16."""
import pytest

from emu import rdna2_exec_pkf16, rdna2_gpu, rdna2_hlsl, rdna2_hlsl_graphics
from emu.rdna2_exec_ops import float_bits
from test_rdna2_exec import V, program, vop1, vop2, vop3
from test_rdna2_exec_pkf16 import LITERAL, pack_f16, vop3p
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def test_hlsl_coverage_includes_all_pkf16_ops():
    covered = rdna2_hlsl.coverage_names()
    assert rdna2_exec_pkf16.ALL_OPS <= covered


def test_hlsl_translates_all_pkf16_ops_in_wave32_wave64_and_graphics():
    code = program(
        vop1("v_mov_b32", 1, LITERAL), pack_f16(1.5, -2.0),
        vop1("v_mov_b32", 2, LITERAL), pack_f16(2.0, 3.0),
        vop1("v_mov_b32", 3, LITERAL), float_bits(0.25),
        *vop3p("v_pk_add_f16", 4, V + 1, V + 2, op_sel=0b001, neg_lo=0b010),
        *vop3p("v_pk_mul_f16", 5, V + 1, V + 2, clamp=1),
        *vop3p("v_pk_fma_f16", 6, 242, V + 1, V + 2),
        *vop3p("v_pk_min_f16", 7, V + 1, V + 2),
        *vop3p("v_pk_max_f16", 8, V + 1, V + 2),
        vop2("v_pk_fmac_f16", 4, V + 1, 2),
        *vop3("v_pk_fmac_f16", 5, V + 1, V + 2, 0, clamp=1),
        *vop3p("v_fma_mix_f32", 9, V + 1, V + 3, V + 2, op_sel=0b100, op_sel_hi=0b101, neg_lo=0b100, neg_hi=0b001,
               clamp=1),
        *vop3p("v_fma_mixlo_f16", 10, V + 1, V + 3, V + 2, op_sel_hi=0b101),
        *vop3p("v_fma_mixhi_f16", 11, V + 1, V + 3, V + 2, op_sel_hi=0b101, clamp=1),
        *vop3p("v_dot2_f32_f16", 12, V + 1, V + 2, V + 3, neg_lo=0b100, clamp=1),
    )
    hlsl32 = rdna2_hlsl.translate(code, wave_size=32)
    hlsl64 = rdna2_hlsl.translate(code, wave_size=64)
    hlsl_vs = rdna2_hlsl_graphics.translate_vertex(code)
    for text in (hlsl32, hlsl64, hlsl_vs):
        assert "uint F32Clamp01(" in text
        assert "F16Add(" in text
        assert "F16Mul(" in text
        assert "F16Fma(" in text
        assert "F16Min(" in text
        assert "F16Max(" in text
        assert "FmaF32(" in text
        assert "F32Clamp01(" in text


def test_hlsl_refuses_same_undefined_modifiers_as_interpreter():
    for bad in (
        program(*vop3p("v_fma_mix_f32", 4, V + 1, V + 2, V + 3, op_sel=0b001, op_sel_hi=0b000)),
        program(*vop3p("v_dot2_f32_f16", 4, V + 1, V + 2, V + 3, neg_hi=0b100)),
        program(*vop3("v_pk_fmac_f16", 4, V + 1, V + 2, 0, op_sel=0b0001)),
    ):
        with pytest.raises(rdna2_hlsl.Untranslatable):
            rdna2_hlsl.translate(bad)


@gpu
def test_pkf16_and_fma_mix_match_interpreter_on_gpu():
    code = with_lane_offset(
        vop1("v_mov_b32", 1, LITERAL), pack_f16(1.5, -2.0),
        vop1("v_mov_b32", 2, LITERAL), pack_f16(2.0, 3.0),
        vop1("v_mov_b32", 3, LITERAL), pack_f16(0.25, -1.0),
        vop1("v_mov_b32", 4, LITERAL), float_bits(4.0),
        vop1("v_mov_b32", 5, LITERAL), 0xBEEF_CAFE,
        vop1("v_mov_b32", 6, LITERAL), 0xBEEF_CAFE,
        *vop3p("v_pk_add_f16", 7, V + 1, V + 2, op_sel=0b001, neg_lo=0b001, neg_hi=0b010),
        *vop3p("v_pk_mul_f16", 8, V + 1, V + 2, clamp=1),
        *vop3p("v_pk_fma_f16", 9, 242, V + 1, V + 3),
        *vop3p("v_pk_min_f16", 10, V + 1, V + 2),
        *vop3p("v_pk_max_f16", 11, V + 1, V + 2),
        vop2("v_pk_fmac_f16", 7, V + 1, 2),
        *vop3("v_pk_fmac_f16", 8, V + 1, V + 2, 0, clamp=1),
        *vop3p("v_fma_mix_f32", 12, V + 1, V + 4, V + 3, op_sel=0b100, op_sel_hi=0b101, neg_lo=0b100, neg_hi=0b001),
        *vop3p("v_fma_mixlo_f16", 5, V + 1, V + 4, V + 3, op_sel_hi=0b101, neg_hi=0b001),
        *vop3p("v_fma_mixhi_f16", 6, V + 1, V + 4, V + 3, op_sel_hi=0b101, clamp=1),
        *vop3p("v_dot2_f32_f16", 13, V + 1, V + 2, V + 4, neg_lo=0b100),
        *store_lane(5, 0), *store_lane(6, 1), *store_lane(7, 2), *store_lane(8, 3),
        *store_lane(9, 4), *store_lane(10, 5), *store_lane(11, 6), *store_lane(12, 7),
        *store_lane(13, 8),
    )
    expected, actual = compare(code)
    assert actual == expected
