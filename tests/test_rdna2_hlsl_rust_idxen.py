"""Untyped MUBUF index and index/offset pairs on raw and structured descriptors."""
import struct

import pytest

from emu import rdna2_hlsl
from test_rdna2_exec import LITERAL, V, const, program, sop1, vop1, vop2
from test_rdna2_exec_memory import INPUT, mubuf
from test_rdna2_hlsl import store_lane, with_lane_offset
from test_rdna2_hlsl_rust_half import verify
from test_rdna2_hlsl_rust_parity import rust


def indexed(name, index, data, offen, glc=False):
    low, high = mubuf(name, index, data, 20, offen=offen, offset=4, soffset=24)
    return low | (1 << 13) | (int(glc) << 14), high


@pytest.mark.parametrize("stride,records,offen", [(16, 9, 0), (16, 9, 1), (0, 60, 1), (0, 7, 0), (0, 12, 0)])
def test_index_and_offset_with_overlapping_load_destination(stride, records, offen):
    words = [sop1("s_mov_b32", 20, LITERAL), INPUT, sop1("s_mov_b32", 21, LITERAL), stride << 16,
             sop1("s_mov_b32", 22, LITERAL), records, sop1("s_mov_b32", 24, const(8)),
             vop1("v_mov_b32", 10, V), vop2("v_lshlrev_b32", 11, const(2), 0),
             sop1("s_mov_b32", 126, LITERAL), 0x55555555,
             *indexed("buffer_load_dwordx2", 10, 10, offen),
             *store_lane(10, 0), *store_lane(11, 1)]
    verify(with_lane_offset(*words), bytes(INPUT) + struct.pack("<256I", *range(1, 257)))


def test_indexed_store_and_atomic_return_value():
    words = [sop1("s_mov_b32", 20, LITERAL), INPUT, sop1("s_mov_b32", 21, LITERAL), 16 << 16,
             sop1("s_mov_b32", 22, const(8)), sop1("s_mov_b32", 24, const(4)),
             vop1("v_mov_b32", 10, V), vop1("v_mov_b32", 11, const(4)),
             vop2("v_add_nc_u32", 3, const(10), 0),
             *indexed("buffer_store_dword", 10, 3, 1), vop1("v_mov_b32", 3, const(7)),
             *indexed("buffer_atomic_add", 10, 3, 1, glc=True), *store_lane(3, 0)]
    verify(with_lane_offset(*words), b"")


def test_structured_index_product_cannot_alias_low_uav_memory():
    words = [sop1("s_mov_b32", 20, LITERAL), INPUT, sop1("s_mov_b32", 21, LITERAL), 16 << 16,
             sop1("s_mov_b32", 22, LITERAL), 0x10000001, vop1("v_mov_b32", 10, LITERAL), 0x10000000,
             *indexed("buffer_load_dword", 10, 6, 0), *store_lane(6, 0)]
    verify(with_lane_offset(*words), bytes(INPUT) + struct.pack("<2I", 11, 29))


def test_idxen_offen_requires_two_address_registers():
    name = "buffer_load_dword"
    data = program(*indexed(name, 255, 10, 1))
    with pytest.raises(rdna2_hlsl.Untranslatable, match=name):
        rdna2_hlsl.translate(data)
    result = rust(data)
    assert result.returncode and name.encode() in result.stderr
