"""VOP3 encodings of VOP1/VOP2 arithmetic and native cube, byte and SAD operations."""
import pytest

from test_rdna2_exec import OPCODES, LITERAL, V, vop1, vop3
from test_rdna2_hlsl import store_lane, with_lane_offset
from test_rdna2_hlsl_rust_half import verify
from test_rdna2_hlsl_rust_vop1 import OPS as UNARY
from test_rdna2_hlsl_rust_vop2 import OPS as BINARY
from test_rdna2_hlsl_rust_vop2_float import OPS as FLOAT


EXTRA = ("v_cubeid_f32", "v_cubesc_f32", "v_cubetc_f32", "v_cubema_f32", "v_lerp_u8", "v_sad_u8",
         "v_sad_hi_u8", "v_sad_u32", "v_mul_lo_i32")
NAMES = sorted(name for name in set(UNARY + BINARY + FLOAT + EXTRA) if ("VOP3", name) in OPCODES
               and "_co_ci_" not in name)


@pytest.mark.parametrize("offset", range(0, len(NAMES), 12))
def test_vop3_results(offset):
    ops = [vop1("v_mov_b32", 1, LITERAL), 0xBF400000, vop1("v_mov_b32", 2, LITERAL), 0x40000000,
           vop1("v_mov_b32", 3, LITERAL), 0x3F000000]
    for i, name in enumerate(NAMES[offset:offset + 12]):
        src2 = 106 if name == "v_cndmask_b32" else V + 3
        ops += [*vop3(name, 6, V + 1, V + 2, src2), *store_lane(6, i)]
    verify(with_lane_offset(*ops), b"")
