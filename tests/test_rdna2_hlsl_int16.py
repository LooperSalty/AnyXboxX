"""16-bit integer VALU: the HLSL translation against the reference interpreter, byte for byte (wave32, wave64).

Every encoding of emu/rdna2_exec_int16.py runs on per-lane boundary halves (0, 1, 0x7FFF, 0x8000, 0xFFFF…)
under junk other halves, with its op_sel / op_sel_hi and clamp variants, constant and literal sources, and a
partial EXEC that must leave the inactive lanes' destinations untouched.
"""
import random
import struct

import pytest

import test_rdna2_hlsl_wave64 as wave64
from emu import (rdna2_exec, rdna2_exec_graphics, rdna2_gpu, rdna2_graphics as graphics, rdna2_hlsl,
                 rdna2_hlsl_graphics)
from test_rdna2_exec import V, const, program, sop1, vop1, vop2, vop3
from test_rdna2_exec_int16 import vop3p
from test_rdna2_exec_memory import INPUT, OUTPUT, global_
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

LITERAL, ONE, MINUS_SIXTEEN, EXEC_LO = 255, 242, 208, 126
SLOTS_PER_BASE = 16                  # 15 × 128 bytes fits the signed 12-bit offset
SLOTS_PER_BASE_64 = 8                # 7 × 256 bytes
FIRST_RESULT = 32                    # v31 holds the store address
PARTIAL_EXEC = 0x0F0F_F0F3
EDGES = (0x0000, 0x0001, 0x0002, 0x000F, 0x0011, 0x7FFE, 0x7FFF, 0x8000, 0x8001, 0xFFFE, 0xFFFF, 0x1234, 0xAAAA,
         0x4000, 0xC000)
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc or anyxboxx_gpu_run.exe missing")

HALF_VOP3 = ("v_add_nc_u16", "v_sub_nc_u16", "v_add_nc_i16", "v_sub_nc_i16", "v_mul_lo_u16", "v_lshlrev_b16",
             "v_lshrrev_b16", "v_ashrrev_i16", "v_max_u16", "v_max_i16", "v_min_u16", "v_min_i16")
THREE_VOP3 = ("v_mad_u16", "v_mad_i16", "v_min3_u16", "v_min3_i16", "v_max3_u16", "v_max3_i16", "v_med3_u16",
              "v_med3_i16")
SATURATING = {"v_add_nc_u16", "v_sub_nc_u16", "v_add_nc_i16", "v_sub_nc_i16", "v_mad_u16", "v_mad_i16"}
PACKED = ("v_pk_add_u16", "v_pk_sub_u16", "v_pk_add_i16", "v_pk_sub_i16", "v_pk_mul_lo_u16", "v_pk_max_u16",
          "v_pk_max_i16", "v_pk_min_u16", "v_pk_min_i16", "v_pk_lshlrev_b16", "v_pk_lshrrev_b16",
          "v_pk_ashrrev_i16", "v_pk_mad_u16", "v_pk_mad_i16")
PACKED_CLAMP = {"v_pk_add_u16", "v_pk_sub_u16", "v_pk_add_i16", "v_pk_sub_i16", "v_pk_mad_u16", "v_pk_mad_i16",
                "v_pk_max_u16", "v_pk_max_i16", "v_pk_min_u16", "v_pk_min_i16"}


def lane_words(seed, count):
    """`count` words whose halves are boundary values (one lane in eight fully random)."""
    generator = random.Random(seed)
    words = []
    for lane in range(count):
        if lane % 8 == 7:
            words.append(generator.getrandbits(32))
        else:
            words.append(generator.choice(EDGES) << 16 | generator.choice(EDGES))
    return words


def parity_memory(count=32):
    """v1, v2, v3 (sources) and v4 (old destination) of each lane, one row of `count` words each."""
    rows = [word for seed in (1, 2, 3, 4) for word in lane_words(seed, count)]
    image = bytearray(INPUT + 4 * len(rows))
    struct.pack_into(f"<{len(rows)}I", image, INPUT, *rows)
    return bytes(image)


def cases():
    """Instruction words, one result register each (in order from FIRST_RESULT)."""
    a, b, c = V + 1, V + 2, V + 3
    found = []
    for name in HALF_VOP3:
        variants = [(0, 0), (0b0001, 0), (0b0010, 0), (0b1011, 0)] + [(0, 1), (0b1001, 1)] * (name in SATURATING)
        found += [vop3(name, 0, a, b, op_sel=op_sel, clamp=clamp) for op_sel, clamp in variants]
    for name in THREE_VOP3:
        variants = [(0, 0), (0b0101, 0), (0b1110, 0)] + [(0, 1), (0b1111, 1)] * (name in SATURATING)
        found += [vop3(name, 0, a, b, c, op_sel=op_sel, clamp=clamp) for op_sel, clamp in variants]
    for name in ("v_mad_u32_u16", "v_mad_i32_i16"):
        found += [vop3(name, 0, a, b, c, op_sel=op_sel, clamp=clamp)
                  for op_sel, clamp in ((0, 0), (0, 1), (0b0011, 0), (0b0001, 1), (0b0010, 0), (0b0011, 1))]
    for name in ("v_cvt_pk_u16_u32", "v_cvt_pk_i16_i32"):
        found += [vop3(name, 0, a, b), vop3(name, 0, b, a)]
    for name in ("v_add_nc_i32", "v_sub_nc_i32"):
        found += [vop3(name, 0, a, b), vop3(name, 0, a, b, clamp=1)]
    for name in PACKED:
        three = name.startswith("v_pk_mad")
        sources = (a, b, c) if three else (a, b)
        variants = [(0, 0b111, 0), (0b011, 0b100, 0), (0b101, 0b010, 0)]
        variants += [(0, 0b111, 1), (0b001, 0b110, 1)] * (name in PACKED_CLAMP)
        found += [vop3p(name, 0, *sources, op_sel=op_sel, op_sel_hi=hi, clamp=clamp)
                  for op_sel, hi, clamp in variants]
    return [(words, ()) for words in found] + constant_cases()


def constant_cases():
    """Inline integers and floats and a literal on 16-bit and packed sources: (words, literal words)."""
    a, b = V + 1, V + 2
    return [
        (vop3("v_add_nc_u16", 0, ONE, a, op_sel=0b0001), ()),
        (vop3("v_max_i16", 0, MINUS_SIXTEEN, a, op_sel=0b1001), ()),
        (vop3("v_mad_i32_i16", 0, a, b, ONE, op_sel=0b0001), ()),
        (vop3("v_mad_u32_u16", 0, const(7), a, LITERAL), (0xFFFF_FFF0,)),
        (vop3("v_lshrrev_b16", 0, LITERAL, a, op_sel=0b0001), (0x0003_0009,)),
        (vop3p("v_pk_add_u16", 0, MINUS_SIXTEEN, a), ()),
        (vop3p("v_pk_mul_lo_u16", 0, ONE, a, op_sel=0b001, op_sel_hi=0b010), ()),
        (vop3p("v_pk_sub_i16", 0, LITERAL, a, clamp=1), (0x8001_7FFF,)),
    ]


def with_vdst(words, vdst):
    low, high = words
    return low & ~0xFF | vdst, high


def operations(count, first=FIRST_RESULT):
    """Load the sources, preset every destination to the lane's old value, run all cases under a partial EXEC."""
    words = [*global_("global_load_dword", addr=31, vdst=1, saddr=0),
             *global_("global_load_dword", addr=31, vdst=2, saddr=0, offset=4 * count),
             *global_("global_load_dword", addr=31, vdst=3, saddr=0, offset=8 * count),
             *global_("global_load_dword", addr=31, vdst=4, saddr=0, offset=12 * count)]
    listed = cases()
    words += [vop1("v_mov_b32", first + index, V + 4) for index in range(len(listed))]
    words += [sop1("s_mov_b32", EXEC_LO, LITERAL), PARTIAL_EXEC]
    if count == 64:
        words += [sop1("s_mov_b32", EXEC_LO + 1, LITERAL), 0xF00F_0FF0]
    for index, (instruction, literal) in enumerate(listed):
        words += [*with_vdst(instruction, first + index), *literal]
    words += [sop1("s_mov_b32", EXEC_LO, 193)] + ([sop1("s_mov_b32", EXEC_LO + 1, 193)] if count == 64 else [])
    return words, len(listed)


def parity_program():
    words, total = operations(32)
    stores = []
    for slot in range(total):
        if slot and slot % SLOTS_PER_BASE == 0:
            stores += [vop2("v_add_nc_u32", 31, LITERAL, 31), SLOTS_PER_BASE * 128]
        stores += store_lane(FIRST_RESULT + slot, slot % SLOTS_PER_BASE)
    return with_lane_offset(*words, *stores), total


def test_parity_program_fits_the_register_file_and_memory():
    _, total = parity_program()
    assert FIRST_RESULT + total <= 256 and total * wave64.SLOT <= 0x10000
    assert total >= 160


def test_hlsl_uses_the_int16_helpers():
    hlsl = rdna2_hlsl.translate(parity_program()[0])
    assert "int Sext16(uint x)" in hlsl and "SatI16(" in hlsl and "AddSatI32(" in hlsl


def test_parity_program_results_vary_by_lane():
    """Guard for the GPU test: most results differ between lanes, and inactive lanes kept their old value."""
    wave = rdna2_exec.Wave()
    wave.s[0], wave.s[2] = INPUT, OUTPUT
    wave.memory.write(0, parity_memory())
    data, total = parity_program()
    rdna2_exec.run(data, wave)
    varied = [len(set(wave.v[FIRST_RESULT + i])) > 8 for i in range(total)]
    assert sum(varied) > 0.9 * total
    assert all(wave.v[FIRST_RESULT + i][2] == wave.v[4][2] for i in range(total))      # lane 2 inactive


@gpu
def test_int16_ops_match_interpreter_on_gpu():
    expected, actual = compare(parity_program()[0], memory=parity_memory())
    assert actual == expected


@gpu
def test_int16_ops_match_interpreter_on_gpu_in_wave64():
    words, total = operations(64)
    stores = []
    for slot in range(total):
        if slot and slot % SLOTS_PER_BASE_64 == 0:
            stores += [vop2("v_add_nc_u32", 31, LITERAL, 31), SLOTS_PER_BASE_64 * wave64.SLOT]
        stores += wave64.store_lane(FIRST_RESULT + slot, slot % SLOTS_PER_BASE_64)
    expected, actual = wave64.compare(wave64.with_lane_offset(*words, *stores), memory=parity_memory(64))
    assert actual == expected


# --- Graphics stages: the int16 helpers are part of the vertex and pixel shader HLSL too -----------------
def int16_vertex_shader():
    """test_rdna2_graphics's vertex shader, with the colour words run through the helper-backed int16 ops."""
    import test_rdna2_graphics as graphics_test
    return program(
        vop2("v_lshlrev_b32", 1, const(5), 0),
        *global_("global_load_dwordx4", addr=1, vdst=2, saddr=0),
        *global_("global_load_dwordx4", addr=1, vdst=6, saddr=0, offset=16),
        *vop3("v_add_nc_i16", 6, V + 6, V + 7, clamp=1),                 # AddSatI16
        *vop3("v_sub_nc_i16", 7, V + 8, V + 9, op_sel=0b1011, clamp=1),  # high halves
        *vop3("v_add_nc_i32", 8, V + 6, V + 9, clamp=1),                 # AddSatI32
        *vop3p("v_pk_sub_i16", 9, V + 7, V + 8, clamp=1),
        *graphics_test.exp(graphics.TARGET_POS0, (2, 3, 4, 5), done=True),
        *graphics_test.exp(graphics.TARGET_PARAM0, (6, 7, 8, 9)))


def test_graphics_hlsl_defines_the_int16_helpers():
    vertex = rdna2_hlsl_graphics.translate_vertex(int16_vertex_shader(), {0: INPUT, 1: 0})
    assert "AddSatI16(" in vertex and "uint AddSatI16(" in vertex and "uint AddSatI32(" in vertex


@gpu
def test_int16_ops_in_a_vertex_shader_match_the_interpreter():
    import test_rdna2_graphics as graphics_test
    shader, sgprs = int16_vertex_shader(), {0: INPUT, 1: 0}
    memory = graphics_test.initial_memory()
    after = rdna2_gpu.draw(rdna2_hlsl_graphics.translate_vertex(shader, sgprs), None, bytes(memory), 6,
                           graphics_test.WIDTH, graphics_test.HEIGHT)
    for vertex in range(6):
        expected = rdna2_exec_graphics.run_vertex(shader, vertex, sgprs, bytes(memory))
        record = graphics.VERTEX_RECORDS + vertex * graphics.VERTEX_STRIDE
        assert graphics_test.words(after, record + 16, 4) == expected[graphics.TARGET_PARAM0], f"vertex {vertex}"
