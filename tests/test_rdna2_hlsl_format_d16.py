"""16-bit typed buffer accesses recompiled to HLSL: the same bytes as the reference interpreter, on GPU or WARP.

Each program fills v8-v11 with lane-dependent values, runs d16 loads (so the halves a load must keep are
visible) and writes v8 and v9 to memory after each load; stores are compared through the memory they
write. Values computed by hand: tests/test_rdna2_exec_format_d16.py.
"""
import random
import struct

import pytest

from emu import rdna2_gpu
import test_rdna2_hlsl_wave64 as wave64
from test_rdna2_exec import const, sop1, vop2
from test_rdna2_exec_format import XYZW, typed
from test_rdna2_hlsl import store_lane
from test_rdna2_hlsl_format import (ELEMENTS, INDEXED, LANES, STRIDE, check, descriptor, element_bytes,
                                    memory_with, store_values)
from test_rdna2_exec_memory import BUFFER

LITERAL, EXEC_LO, MINUS_ONE = 255, 126, 193          # 193: inline constant -1
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")

# float32 values worth narrowing: rounding ties, overflow, subnormals, infinities, NaN with payload.
SPECIAL_FLOATS = [0x3F800000, 0xC2280000, 0x477FE000, 0x477FEFFF, 0x477FF000, 0x501502F9, 0xFF800000,
                  0x80000000, 0x00000001, 0x807FFFFF, 0x3F801000, 0x3F803000, 0x38800000, 0x387FC000,
                  0x33800000, 0x33000000, 0x33000001, 0x33C00000, 0x7FC00000, 0x7F802000, 0xFFC00001,
                  0x3F7FF000, 0x3F7FB000, 0x7F7FFFFF, 0x37FFFFFF, 0xB3800001, 0x7F800001, 0xFF800002,
                  0xC77FF000]


def float_elements(seed):
    """32 elements of four float32: the special values first, then random bits."""
    generator = random.Random(seed)
    words = list(SPECIAL_FLOATS)
    while len(words) < LANES * 4:
        words.append(generator.getrandbits(32))
    generator.shuffle(words)
    return struct.pack(f"<{len(words)}I", *words)


def refill():
    """v8-v11: lane-dependent values that a d16 load must keep in the halves it does not write."""
    return store_values()


def d16_loads(specs, exec_mask=None):
    """(name, V# n°, options) → refill v8-v11, the load into v8… (under EXEC = `exec_mask` if given),
    then v8 and v9 in two output slots each, all lanes active."""
    words, slot = [], 0
    for name, rsrc_index, options in specs:
        words += refill()
        if exec_mask is not None:
            words += [sop1("s_mov_b32", EXEC_LO, LITERAL), exec_mask]
        words += typed(name, rsrc=24 + 4 * rsrc_index, **options)
        if exec_mask is not None:
            words += [sop1("s_mov_b32", EXEC_LO, MINUS_ONE)]
        words += store_lane(8, slot) + store_lane(9, slot + 1)
        slot += 2
    assert slot <= 31
    return words


@gpu
def test_float_narrowing_matches():
    # V# 0: 32_32_32_32_FLOAT, V# 1: 32_FLOAT (byte offset = lane × 16 + 4), V# 2: 16_16_16_16_FLOAT;
    # tbuffer 32_32_32_FLOAT (xyz: v9's high half kept) and 32_32_FLOAT.
    offset = {"vaddr": 1, "offen": 1, "offset": 4}
    words = [vop2("v_lshlrev_b32", 1, const(4), 0)]
    words += d16_loads([("buffer_load_format_d16_xyzw", 0, INDEXED),
                        ("buffer_load_format_d16_hi_x", 1, offset),
                        ("buffer_load_format_d16_x", 1, {**offset, "offset": 8}),
                        ("tbuffer_load_format_d16_xyz", 0, {**INDEXED, "fmt": 74}),
                        ("tbuffer_load_format_d16_xy", 0, {**INDEXED, "fmt": 64, "offset": 8}),
                        ("buffer_load_format_d16_xyzw", 2, INDEXED)])
    descriptors = [descriptor(77), descriptor(22, records=LANES * STRIDE, stride=0, selects=4),
                   descriptor(71)]
    check(words, descriptors, memory_with((BUFFER, float_elements(7))))


def normalized_elements(seed):
    """element_bytes plus, on lanes 4-7, the 16-bit fields whose float32 value is a float16 midpoint and
    the four values that separate one rounding from two (0xFFAF, 0xFFEF, 0x7FF7, 0x8009:
    tests/test_rdna2_exec_format_d16.py)."""
    data = bytearray(element_bytes(seed))
    data[64:80] = struct.pack("<8H", 65455, 65519, 32759, 0x8001, 65535, 2049, 2051, 65520)
    data[80:96] = struct.pack("<8H", 13107, 1, 0x7FFF, 0x8000, 0xFFFF, 0x7C01, 0x0400, 0x03FF)
    data[96:112] = bytes([51, 128, 255, 0, 0x80, 0x81, 0x7F, 0x01, 0x00, 0x02, 0xFF, 0xFE, 1, 2, 3, 4])
    data[112:128] = struct.pack("<8H", 0x8009, 0xFFAF, 0xFFEF, 0x7FF7, 0x7FF7, 0x8009, 0xFFAF, 0xFFEF)
    return bytes(data)


@gpu
@pytest.mark.parametrize("formats", [
    (65, 66, 67, 68),            # 16_16_16_16_UNORM/SNORM/USCALED/SSCALED
    (69, 70, 56, 57),            # 16_16_16_16_UINT/SINT, 8_8_8_8_UNORM/SNORM
    (50, 51, 49, 61),            # 2_10_10_10_UNORM/SNORM, 10_10_10_2_SINT, 8_8_8_8_SINT
    (58, 59, 47, 55),            # 8_8_8_8_USCALED/SSCALED, 10_10_10_2_SSCALED, 2_10_10_10_SINT
])
def test_normalized_scaled_and_integer_formats_match(formats):
    words = d16_loads([("buffer_load_format_d16_xyzw", i, INDEXED) for i in range(4)]
                      + [("buffer_load_format_d16_xyz", 0, INDEXED), ("buffer_load_format_d16_hi_x", 1, INDEXED)])
    check(words, [descriptor(fmt) for fmt in formats], memory_with((BUFFER, normalized_elements(len(words)))))


@gpu
def test_dst_sel_bounds_tbuffer_and_partial_exec_match():
    # V# 0: 8_8_8_8_UNORM, 20 elements, DST_SEL W, 1, X, 0; V# 1: 8_8_8_8_UINT, same selects;
    # V# 2: 16_16_SINT with DST_SEL Y, X; V# 3: FORMAT INVALID for a tbuffer (zeros). EXEC = 0x8F0F00F3
    # during the loads, then all lanes for the stores: inactive lanes keep the refilled values.
    selects = 7 | 1 << 3 | 4 << 6 | 0 << 9
    words = d16_loads([("buffer_load_format_d16_xyzw", 0, INDEXED),
                        ("buffer_load_format_d16_xyz", 1, INDEXED),
                        ("buffer_load_format_d16_xy", 2, INDEXED),
                        ("tbuffer_load_format_d16_xyzw", 0, {**INDEXED, "fmt": 71}),
                        ("tbuffer_load_format_d16_xy", 3, {**INDEXED, "fmt": 29})], exec_mask=0x8F0F00F3)
    descriptors = [descriptor(56, records=20, selects=selects), descriptor(60, selects=selects),
                   descriptor(28, selects=5 | 4 << 3), descriptor(0)]
    check(words, descriptors, memory_with((BUFFER, normalized_elements(9))))


@gpu
def test_d16_stores_match():
    # V# 0: 8_UINT, stride 1 (four lanes per word: partial writes); V# 1: 16_FLOAT, stride 2 (half words);
    # V# 2: 8_8_8_8_SINT, 30 elements; V# 3: 32_32_FLOAT, stride 8; V# 4: 32_FLOAT, stride 4 (hi_x);
    # tbuffer 32_32_32_SINT (xyz: z from bits 15-0 of v9) and 16_16_16_16_UINT through V# 5 (stride 16,
    # so that elements of different lanes never overlap).
    words = store_values() + [sop1("s_mov_b32", EXEC_LO, LITERAL), 0xFFFF7FFE]
    words += typed("buffer_store_format_d16_x", vaddr=0, idxen=1, rsrc=24)
    words += typed("buffer_store_format_d16_x", vaddr=0, vdata=9, idxen=1, rsrc=28)
    words += typed("buffer_store_format_d16_xyzw", vaddr=0, idxen=1, rsrc=32)
    words += typed("buffer_store_format_d16_xy", vaddr=0, vdata=10, idxen=1, rsrc=36)
    words += typed("buffer_store_format_d16_hi_x", vaddr=0, vdata=11, idxen=1, rsrc=40)
    words += typed("tbuffer_store_format_d16_xyz", vaddr=0, idxen=1, rsrc=44, fmt=73)
    words += typed("tbuffer_store_format_d16_xyzw", vaddr=0, vdata=9, idxen=1, rsrc=44, fmt=69, offset=0x200)
    descriptors = [descriptor(5, stride=1, selects=4, base=ELEMENTS),
                   descriptor(13, stride=2, selects=4, base=ELEMENTS + 0x100),
                   descriptor(61, records=30, stride=4, base=ELEMENTS + 0x200),
                   descriptor(64, stride=8, selects=4 | 5 << 3 | 1 << 6 | 1 << 9, base=ELEMENTS + 0xA00),
                   descriptor(22, stride=4, selects=4, base=ELEMENTS + 0xC00),
                   descriptor(22, stride=16, selects=0, base=ELEMENTS + 0x400)]
    check(words, descriptors, memory_with((ELEMENTS, bytes([0xA5]) * 0x1000)))


@gpu
def test_wave64_d16_load_and_store_match():
    # V# in s20-s23: 16_16_16_16_SNORM, stride 8, 60 elements (lanes 60-63 out of the buffer);
    # d16_xyz keeps v9's high half; tbuffer 8_8_8_8_SINT stores (v10, v11) halves at BUFFER + 0x800.
    words = [sop1("s_mov_b32", 21, LITERAL), 8 << 16, sop1("s_mov_b32", 22, const(60)),
             sop1("s_mov_b32", 23, LITERAL), 66 << 12 | XYZW, *store_values(),
             *typed("buffer_load_format_d16_xyz", vaddr=0, idxen=1, rsrc=20),
             *[word for c in range(2) for word in wave64.store_lane(8 + c, c)],
             *typed("tbuffer_store_format_d16_xyzw", vaddr=0, vdata=10, idxen=1, rsrc=20, fmt=61, offset=0x800)]
    generator = random.Random(8)
    memory = memory_with((BUFFER, bytes(generator.getrandbits(8) for _ in range(64 * 8))))
    expected, actual = wave64.compare(wave64.with_lane_offset(*words), memory)
    assert actual == expected
