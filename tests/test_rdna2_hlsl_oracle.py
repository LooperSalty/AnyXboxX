"""Exact f32 results and named indexed-register refusals in both emitters."""
import struct

import pytest

from emu import rdna2_exec, rdna2_hlsl
from test_rdna2_exec import LITERAL, V, const, program, sop1, sopp, vop1, vop3
from test_rdna2_exec_memory import INPUT, global_
from test_rdna2_hlsl import store_lane, with_lane_offset
from test_rdna2_hlsl_rust_half import verify
from test_rdna2_hlsl_rust_parity import rust
from test_rdna2_sdwa_dpp_exec import SDWA, sdwa_word

FOLDED = ("v_sin_f32", "v_cos_f32", "v_exp_f32", "v_log_f32")
RUNTIME = ("v_rsq_f32", "v_sqrt_f32", "v_rcp_f32", "v_rcp_iflag_f32")


def refused(data, name):
    with pytest.raises(rdna2_hlsl.Untranslatable, match=name):
        rdna2_hlsl.translate(data)
    result = rust(data)
    assert result.returncode and name.encode() in result.stderr


@pytest.mark.parametrize("name", FOLDED)
def test_runtime_transcendental_requires_exact_implementation(name):
    data = program(vop1(name, 2, V + 1))
    rdna2_exec.run(data)
    refused(data, name)


def test_transcendental_constants_match_reference_on_warp():
    ops = []
    for i, name in enumerate(FOLDED):
        ops += [vop1(name, 2, LITERAL), 0x3e800000, *store_lane(2, 2*i)]
        ops += [*vop3(name, 2, LITERAL, neg=1), 0x3e800000, *store_lane(2, 2*i + 1)]
    verify(with_lane_offset(*ops), b"")


def test_exact_unary_results_with_register_inputs():
    values = [0, 0x80000000, 1, 0x80000001, 0x7fffff, 0x800000, 0x800001, 0x1000000,
              0x3e800000, 0x3eaaaaab, 0x3f7fffff, 0x3f800000, 0x3f800001, 0x3fffffff,
              0x40000000, 0x40000001, 0x40400000, 0xbf800000, 0x7e800000, 0x7e800001,
              0x7f000000, 0x7f7fffff, 0xff7fffff, 0x7f800000, 0xff800000, 0xff812345,
              0x7fc12345, 0x00200000, 0x001fffff, 0x00200001, 0x3456789a, 0xabcdef01]
    ops = list(global_("global_load_dword", 31, vdst=1, saddr=0))
    for i, name in enumerate(RUNTIME):
        ops += [vop1(name, 2, V + 1), *store_lane(2, 3*i),
                *vop3(name, 2, V + 1, neg=1, clamp=1, omod=3), *store_lane(2, 3*i + 1),
                vop1(name, 2, SDWA), sdwa_word(1, clamp=1, omod=1), *store_lane(2, 3*i + 2)]
    verify(with_lane_offset(*ops), bytes(INPUT) + struct.pack("<32I", *values))


@pytest.mark.parametrize("name,offset,dst,src", [
    ("v_movrels_b32", 250, 1, V + 10), ("v_movreld_b32", 0xffffffff, 1, V),
    ("v_movrelsd_b32", 6, 250, V), ("v_movrelsd_2_b32", 257, 0, V),
    ("v_swaprel_b32", 256 << 16, 0, V), ("s_movrels_b32", 0xfffffffd, 1, 47),
    ("s_movrels_b64", 1, 2, 10), ("s_movreld_b32", 100, 10, const(1)),
    ("s_movreld_b64", 2, 104, 0), ("s_movrelsd_2_b32", 106 << 16, 0, 0),
])
def test_indexed_refusals_even_with_empty_exec(name, offset, dst, src):
    encodings = [(sop1(name, dst, src),)] if name.startswith("s_") else [
        (vop1(name, dst, src),), vop3(name, dst, src)]
    for words in encodings:
        data = program(sop1("s_mov_b32", 124, LITERAL), offset,
                       sop1("s_mov_b32", 126, const(0)), *words)
        with pytest.raises(rdna2_exec.Untranslated, match=name):
            rdna2_exec.run(data)
        refused(data, name)


def test_moves_follow_m0_across_branches_and_loops():
    from test_rdna2_exec import sop2

    name = "v_movreld_b32"
    data = with_lane_offset(sop1("s_mov_b32", 124, const(2)),
                            sop1("s_mov_b32", 5, const(2)), sopp("s_branch", 1),
                            sop1("s_mov_b32", 124, const(9)),
                            vop1(name, 10, V), sop2("s_sub_u32", 5, 5, const(1)),
                            sopp("s_cbranch_scc1", -3), *store_lane(12, 0))
    verify(data, b"")


def test_unproven_m0_is_refused_after_branch_or_dynamic_write():
    name = "v_movrels_b32"
    for prefix in [(sop1("s_mov_b32", 124, LITERAL), 250, sopp("s_branch", 1),
                    sop1("s_mov_b32", 124, const(0))),
                   (vop1("v_mov_b32", 2, LITERAL), 250, vop1("v_readfirstlane_b32", 124, V + 2))]:
        data = program(*prefix, vop1(name, 1, V + 10))
        with pytest.raises(rdna2_exec.Untranslated, match=name):
            rdna2_exec.run(data)
        refused(data, name)
