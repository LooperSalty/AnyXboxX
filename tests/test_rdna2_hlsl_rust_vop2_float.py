"""VOP2 floating-point HLSL text and WARP parity."""
import struct
import random

import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import LITERAL, V, program, sop1, vop1, vop2
from test_rdna2_hlsl_rust_parity import rust
from test_rdna2_exec_memory import INPUT, global_
from test_rdna2_hlsl import store_lane, with_lane_offset
from test_rdna2_hlsl_rust_half import verify

OPS = ("v_mul_legacy_f32", "v_fmac_f32", "v_fmac_legacy_f32", "v_mac_f32", "v_add_f32", "v_sub_f32",
       "v_subrev_f32", "v_mul_f32", "v_min_f32", "v_max_f32")


@pytest.mark.parametrize("name", OPS)
@pytest.mark.parametrize("a,b,c,mask", [(0x3fc00000, 0x40000000, 0xbf800000, 0xffffffff),
                                         (0, 0x7f800000, 0x3f800000, 0x55555555),
                                         (0x80000000, 0, 0, 0xaaaaaaaa),
                                         (0x7fc01234, 0x3f800000, 0, 0xffffffff),
                                         (1, 0x3f000000, 0x3f800000, 0xffffffff),
                                         (0x3f800000, 0x7fc01234, 0, 0xffffffff),
                                         (0x7f800000, 0xff800000, 0, 0xffffffff),
                                         (0x7f800000, 0x7f800000, 0, 0xffffffff)])
def test_vop2_float(name, a, b, c, mask, monkeypatch):
    if not rdna2_gpu.available():
        pytest.skip("dxc or GPU runner is unavailable")
    monkeypatch.setenv("ANYXBOXX_GPU", "warp")
    data = program(vop1("v_mov_b32", 1, LITERAL), a, vop1("v_mov_b32", 2, LITERAL), b,
                   vop1("v_mov_b32", 3, LITERAL), c, sop1("s_mov_b32", 126, LITERAL), mask,
                   vop2(name, 3, V + 1, 2))
    result = rust(data)
    assert result.returncode == 0, result.stderr.decode()
    hlsl = result.stdout.decode("utf-8")
    assert hlsl == rdna2_hlsl.translate(data)
    wave = rdna2_exec.run(data)
    hlsl = hlsl.replace("  return;\n", "  g_memory.Store(lane * 4u, v[3]);\n  return;\n")
    assert rdna2_gpu.run(hlsl, bytes(128)) == struct.pack("<32I", *wave.v[3])


@pytest.mark.parametrize("seed", range(8))
def test_finite_f32_ops_round_lane_varying_normal_and_subnormal_values(seed):
    rng = random.Random(seed)
    operands = [rng.getrandbits(32) for _ in range(64)]
    for i in range(0, 64, 3):
        operands[i] &= 0x807FFFFF
    for i in range(1, 32, 5):
        operands[i + 32] = operands[i] ^ 0x80000001
    ops = [*global_("global_load_dword", addr=31, vdst=1, saddr=0),
           *global_("global_load_dword", addr=31, vdst=2, saddr=0, offset=128)]
    for slot, name in enumerate(("v_add_f32", "v_sub_f32", "v_mul_f32", "v_min_f32", "v_max_f32")):
        ops += [vop2(name, 3, V + 1, 2), *store_lane(3, slot)]
    verify(with_lane_offset(*ops), bytes(INPUT) + struct.pack("<64I", *operands))


