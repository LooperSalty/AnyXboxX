"""VOP3b carry-in, carry-out, saturation and 64-bit multiplication in both wave sizes."""

import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import LITERAL, V, sop1, vop1, vop3
from test_rdna2_exec_memory import OUTPUT
from test_rdna2_hlsl import MEMORY_SIZE, store_lane, with_lane_offset
from test_rdna2_hlsl_rust_parity import rust
from test_rdna2_hlsl_wave64 import store_lane as store64


@pytest.mark.parametrize("lanes", [32, 64])
def test_vop3b_arithmetic(lanes):
    store = store_lane if lanes == 32 else store64
    ops = [sop1("s_mov_b32", 2, LITERAL), OUTPUT, sop1("s_mov_b32", 8, LITERAL), 0xFF00F103,
           sop1("s_mov_b32", 9, LITERAL), 0x835FB002, vop1("v_mov_b32", 1, LITERAL), 0xFFFFFFFF]
    for i, op in enumerate(("add", "sub", "subrev")):
        for carry_in in (False, True):
            name = f"v_{op}_co{'_ci' if carry_in else ''}_u32"
            ops += [*vop3(name, 3, V + 1, V, 8, sdst=12, clamp=1),
                    *store(3, 0), vop1("v_mov_b32", 4, 12), *store(4, 1),
                    vop1("v_mov_b32", 4, 13), *store(4, 2)]
            # Move the output base after each group so every result stays observable.
            ops += [sop1("s_mov_b32", 2, LITERAL), OUTPUT + (2 * i + int(carry_in) + 1) * 4096]
    ops += [*vop3("v_mad_u64_u32", 4, V, V + 1, V, sdst=12), *store(4, 0), *store(5, 1),
            *vop3("v_mad_i64_i32", 6, V, V + 1, V, sdst=125), *store(6, 2), *store(7, 3)]
    data = with_lane_offset(*ops)
    result = rust(data, *(["--wave64"] if lanes == 64 else []))
    assert result.returncode == 0, result.stderr.decode()
    hlsl = result.stdout.decode()
    assert hlsl == rdna2_hlsl.translate(data, wave_size=lanes)
    if not rdna2_gpu.available():
        pytest.skip("dxc or GPU runner unavailable")
    wave = rdna2_exec.Wave(lanes=lanes)
    initial = bytes(MEMORY_SIZE)
    wave.memory.write(0, initial)
    rdna2_exec.run(data, wave)
    assert rdna2_gpu.run(hlsl, initial) == wave.memory.read(0, MEMORY_SIZE)
