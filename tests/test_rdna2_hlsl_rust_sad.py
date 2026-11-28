"""Masked and sliding-window SAD with lane-varying reference bytes and accumulators."""
import struct

from test_rdna2_exec import LITERAL, sop1
from test_rdna2_exec_sad import sad_program
from test_rdna2_hlsl import store_lane, with_lane_offset
from test_rdna2_hlsl_rust_half import verify


def test_sad_text_and_warp_memory():
    scalars = (sop1("s_mov_b32", 20, LITERAL), 0x01020304, sop1("s_mov_b32", 21, LITERAL), 0x80706050)
    verify(struct.pack("<4I", *scalars) + sad_program(with_lane_offset, store_lane), b"")
