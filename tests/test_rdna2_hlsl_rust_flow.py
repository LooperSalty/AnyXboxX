"""Wave32 basic blocks, PC reads and scheduling hints against both oracles."""
import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import LITERAL, const, program, sop1, sop2, sopc, sopp, vop1
from test_rdna2_exec_memory import ds, mubuf, smem
from test_rdna2_exec_sopp import NO_EFFECT
from test_rdna2_hlsl import store_lane, with_lane_offset
from test_rdna2_hlsl_rust_parity import rust


def check(data):
    result = rust(data)
    assert result.returncode == 0, result.stderr.decode()
    hlsl = result.stdout.decode()
    assert hlsl == rdna2_hlsl.translate(data)
    if not rdna2_gpu.available():
        pytest.skip("dxc or GPU runner is unavailable")
    start = bytes([0xba]) * 256
    wave = rdna2_exec.Wave()
    wave.memory.write(0, start)
    rdna2_exec.run(data, wave)
    assert rdna2_gpu.run(hlsl, start) == wave.memory.read(0, len(start))


@pytest.mark.parametrize("name", ["s_branch", "s_cbranch_scc0", "s_cbranch_scc1",
                                  "s_cbranch_vccz", "s_cbranch_vccnz",
                                  "s_cbranch_execz", "s_cbranch_execnz"])
@pytest.mark.parametrize("flag", [0, 1])
def test_conditional_and_unconditional_branches(name, flag):
    check(with_lane_offset(sopc("s_cmp_eq_u32", const(flag), const(1)),
                           sop1("s_mov_b32", 106, const(flag)), sop1("s_mov_b32", 126, const(flag)),
                           sop1("s_mov_b32", 4, const(17)), sopp(name, 1),
                           sop1("s_mov_b32", 4, const(29)), sop1("s_mov_b32", 126, LITERAL), 0xffffffff,
                           vop1("v_mov_b32", 1, 4), *store_lane(1, 0)))


def test_loop_pc_and_early_end():
    check(with_lane_offset(sop1("s_mov_b32", 4, const(3)), sop1("s_getpc_b64", 6, 0),
                           sop2("s_add_u32", 5, 5, 6), sop2("s_sub_u32", 4, 4, const(1)),
                           sopc("s_cmp_lg_u32", 4, const(0)), sopp("s_cbranch_scc1", -3),
                           vop1("v_mov_b32", 1, 5), *store_lane(1, 0), sopp("s_endpgm"),
                           *store_lane(0, 0)))


def test_hints_keep_memory_and_registers():
    words = [sopp(name, value) for name, value in NO_EFFECT]
    words.extend(sopp(name, 0) for name in ("s_waitcnt", "s_barrier", "s_sleep", "s_setprio",
                                            "s_ttracedata", "s_clause", "s_inst_prefetch"))
    for name in ("s_dcache_inv", "s_gl1_inv", "s_dcache_wb", "s_atc_probe", "s_atc_probe_buffer"):
        words.extend(smem(name, 4, 2))
    for name in ("buffer_gl0_inv", "buffer_gl1_inv"):
        words.extend(mubuf(name, 0, 0, 0))
    check(with_lane_offset(vop1("v_mov_b32", 1, const(7)), *words, *ds("ds_nop", 0), *store_lane(1, 0)))


@pytest.mark.parametrize("displacement", [-2, 1, 12])
def test_branch_outside_or_into_literal_is_named(displacement):
    data = program(sopp("s_branch", displacement), sop1("s_mov_b32", 4, LITERAL), 1)
    result = rust(data)
    assert result.returncode and b"s_branch" in result.stderr
