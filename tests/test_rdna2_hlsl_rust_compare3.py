"""Every Python-translated VOP3 comparison with lane-varying values and partial EXEC."""
import struct

import pytest

from emu import rdna2_hlsl
from test_rdna2_exec import OPCODES, LITERAL, V, program, sop1, vop1, vop3
from test_rdna2_exec_memory import INPUT, global_
from test_rdna2_hlsl import store_lane, with_lane_offset
from test_rdna2_hlsl_rust_half import verify


def compare_names():
    names = []
    for family, name in sorted(OPCODES):
        if family != "VOP3" or not name.startswith("v_cmp"):
            continue
        try:
            rdna2_hlsl.translate(program(*vop3(name, 8, V + 1, V + 2)))
        except rdna2_hlsl.Untranslatable:
            continue
        names.append(name)
    return names


NAMES = compare_names()


@pytest.mark.parametrize("offset", range(0, len(NAMES), 12))
def test_compare_masks(offset):
    ops = [*global_("global_load_dword", addr=31, vdst=1, saddr=0),
           *global_("global_load_dword", addr=31, vdst=2, saddr=0, offset=128)]
    for i, name in enumerate(NAMES[offset:offset + 12]):
        ops += [sop1("s_mov_b32", 126, LITERAL), 0xFC0FF00F, *vop3(name, 8, V + 1, V + 2),
                sop1("s_mov_b32", 10, 126 if name.startswith("v_cmpx") else 8),
                sop1("s_mov_b32", 126, 193), vop1("v_mov_b32", 3, 10), *store_lane(3, i)]
    words = [0, 0x80000000, 0x7FC07E01, 0xBF80BC00, 0x3F803C00, 2, 0x7F807C00, 0xFFFFFE00]
    memory = bytes(INPUT) + struct.pack("<64I", *(words * 4 + list(reversed(words)) * 4))
    verify(with_lane_offset(*ops), memory)
