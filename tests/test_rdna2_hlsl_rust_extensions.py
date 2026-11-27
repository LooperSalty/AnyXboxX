"""SDWA and DPP read a different subword or lane for each result."""
from test_rdna2_exec import LITERAL, sop1, vop1, vop2, vopc
from test_rdna2_hlsl import store_lane, with_lane_offset
from test_rdna2_exec_ds_addtid import addtid_program
from test_rdna2_hlsl_rust_half import verify
from test_rdna2_sdwa_dpp_exec import SDWA, DPP16, DPP8, DPP8_FI, dpp16_word, dpp8_word, sdwa_word


def test_lds_addtid_uses_m0_and_active_lane():
    verify(with_lane_offset(*addtid_program(32), *store_lane(2, 0)), b"")


def test_subword_and_lane_controls_match_reference_memory():
    ops = [vop2("v_mul_u32_u24", 1, LITERAL, 0), 0xF187,
           vop1("v_mov_b32", 2, LITERAL), 0xFF00FF00,
           sop1("s_mov_b32", 126, LITERAL), 0xF00F0F7F,
           vop2("v_add_nc_u32", 2, SDWA, 1), sdwa_word(1, dst_sel=2, dst_unused=2, src0_sel=1, src0_sext=1),
           vop1("v_cvt_f32_i32", 3, SDWA), sdwa_word(1, src0_sel=0, src0_sext=1, omod=1, clamp=1),
           vopc("v_cmp_eq_u32", SDWA, 1), sdwa_word(1, src0_sel=1, src1_sel=0),
           vop1("v_mov_b32", 4, 106)]
    for reg, ctrl in enumerate([0x1B, 0x101, 0x111, 0x123, 0x140, 0x141, 0x156, 0x162], 5):
        ops += [vop1("v_mov_b32", reg, DPP16), dpp16_word(1, ctrl, fi=reg & 1, bound_ctrl=reg >> 1 & 1)]
    for reg, src in [(13, DPP8), (14, DPP8_FI)]:
        ops += [vop2("v_xor_b32", reg, src, 1), dpp8_word(1, list(reversed(range(8))))]
    ops += [sop1("s_mov_b32", 126, 193)]
    for reg in range(2, 15):
        ops += store_lane(reg, reg - 2)
    verify(with_lane_offset(*ops), b"")
