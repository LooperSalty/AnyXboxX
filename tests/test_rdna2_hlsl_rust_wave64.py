"""The 64-thread compute group reads and writes both halves of each lane mask."""
import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import LITERAL, V, const, sop1, sopp, vop1, vop2, vop3, vopc
from test_rdna2_exec_memory import OUTPUT, ds
from test_rdna2_exec_ds_addtid import addtid_program
from test_rdna2_hlsl import MEMORY_SIZE
from test_rdna2_hlsl_rust_parity import rust
from test_rdna2_hlsl_wave64 import store_lane, with_lane_offset


@pytest.mark.parametrize("ops", [
    [*addtid_program(64), vop1("v_mov_b32", 8, V + 2)],
    [*vop3("v_cmp_gt_u32", 12, V, const(41)), vop1("v_mov_b32", 8, 12), vop1("v_mov_b32", 9, 13),
     *vop3("v_cmp_lt_f16", 12, V, 242, abs_=1, neg=1), vop1("v_mov_b32", 10, 13)],
    [vopc("v_cmp_lt_u32", const(30), 0), vop2("v_cndmask_b32", 8, const(1), 0),
     vop2("v_add_co_ci_u32", 9, LITERAL, 8), 0xFFFFFFFF, vop1("v_mov_b32", 10, 107)],
    [sop1("s_mov_b32", 126, const(0)), sopp("s_cbranch_execz", 1), vop1("v_mov_b32", 8, V),
     vop1("v_readfirstlane_b32", 6, V), sop1("s_mov_b32", 126, 193), vop1("v_mov_b32", 9, 6),
     *vop3("v_readlane_b32", 7, V, const(43)), vop1("v_mov_b32", 10, 7)],
    [*vop3("v_mbcnt_lo_u32_b32", 8, 193, V), *vop3("v_mbcnt_hi_u32_b32", 9, 193, V),
     *ds("ds_bpermute_b32", addr=31, data0=0, vdst=10, offset0=4)],
])
def test_wave64_text_and_full_memory(ops):
    data = with_lane_offset(sop1("s_mov_b32", 2, LITERAL), OUTPUT, *ops,
                            *store_lane(8, 0), *store_lane(9, 1), *store_lane(10, 2))
    result = rust(data, "--wave64")
    assert result.returncode == 0, result.stderr.decode()
    hlsl = result.stdout.decode()
    assert hlsl == rdna2_hlsl.translate(data, wave_size=64)
    if not rdna2_gpu.available():
        pytest.skip("dxc or GPU runner unavailable")
    wave = rdna2_exec.Wave(lanes=64)
    initial = bytes(MEMORY_SIZE)
    wave.memory.write(0, initial)
    rdna2_exec.run(data, wave)
    assert rdna2_gpu.run(hlsl, initial) == wave.memory.read(0, MEMORY_SIZE)
