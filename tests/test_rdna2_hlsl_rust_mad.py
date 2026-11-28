"""MAD double rounding and the four literal multiply-add operations."""
import struct

import pytest

from test_rdna2_exec import LITERAL, V, vop1, vop2, vop3
from test_rdna2_exec_fma import DIRECTED_TRIPLES, SWEEP
from test_rdna2_exec_memory import INPUT, global_
from test_rdna2_hlsl import store_lane, with_lane_offset
from test_rdna2_hlsl_rust_half import verify


@pytest.mark.parametrize("block", range(4))
def test_mad_fma_and_literal_forms_on_lane_varying_inputs(block):
    triples = DIRECTED_TRIPLES + SWEEP
    batch = [triples[(32 * block + i) % len(triples)][:3] for i in range(32)]
    inputs = bytes(INPUT) + b"".join(struct.pack("<32I", *(row[i] for row in batch)) for i in range(3))
    ops = [word for i in range(3) for word in global_("global_load_dword", 31, vdst=i + 1, saddr=0, offset=128*i)]
    ops += [*vop3("v_mad_f32", 4, V + 1, V + 2, V + 3), *store_lane(4, 0),
            vop1("v_mov_b32", 4, V + 3), vop2("v_mac_f32", 4, V + 1, 2), *store_lane(4, 1)]
    for i, name in enumerate(("v_madmk_f32", "v_madak_f32", "v_fmamk_f32", "v_fmaak_f32")):
        k = (0x3F7FFFFF, 0x99800000, 0x7FC00C0C, 0xBF800000)[block]
        ops += [vop2(name, 4, V + 1, 2), k, *store_lane(4, 2*i + 2)]
        ops += [*vop3(name, 4, V + 1, V + 2, LITERAL), k, *store_lane(4, 2*i + 3)]
    verify(with_lane_offset(*ops), inputs)


def test_round_to_nearest_even_preserves_signed_zero_and_nan_payloads():
    words = [0, 0x80000000, 1, 0x80000001, 0x7F800000, 0xFF800000, 0x7FC12345, 0xFF812345,
             0x3F000000, 0x3FC00000, 0x40200000, 0xC0200000, 0xBFC00000, 0xBF000000,
             0x4AFFFFFF, 0x4B000000] * 2
    data = with_lane_offset(*global_("global_load_dword", 31, vdst=1, saddr=0),
                            vop1("v_rndne_f32", 2, V + 1), *vop3("v_rndne_f32", 3, V + 1, neg=1),
                            *store_lane(2, 0), *store_lane(3, 1))
    verify(data, bytes(INPUT) + struct.pack("<32I", *words))
