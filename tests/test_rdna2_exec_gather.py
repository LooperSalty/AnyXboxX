"""image_gather4 with compare, bias or clamp in the reference interpreter: values computed by hand.

Depth grid 2x2 (R channel): (0, 0) = 0.2, (1, 0) = 0.4, (0, 1) = 0.6, (1, 1) = 0.8. At s = t = 0.5 the
footprint is the whole grid and the gather order is (x0, y1), (x1, y1), (x1, y0), (x0, y0), so the R values
come back as 0.6, 0.8, 0.4, 0.2. GPU parity: tests/test_rdna2_hlsl_gather.py.
"""
import struct

import pytest

from emu import rdna2_exec, rdna2_exec_gather as gathers, rdna2_hlsl, rdna2_image as images
from emu.rdna2_exec_ops import float_bits
from emu.rdna2_hlsl_ops import Untranslatable
from emu.rdna2_operands_mimg import address_count
from emu.rdna2_ops import MNEMONICS
from test_rdna2_exec import program, vop1
from test_rdna2_exec_memory import OUTPUT, global_
from test_rdna2_graphics import words
from test_rdna2_image import LITERAL, MEMORY_SIZE, S_SHARP, T_SHARP, TEXTURE, mimg, run_interpreter

ZERO, HALF = 128, 0xF0
ONE = images.ONE_BITS
QNAN, DENORMAL, NEG_ZERO = 0x7FC00000, 0x00000001, 0x80000000
COMPARE_OPS = sorted(name for name in gathers.OPERATIONS if gathers.is_compare(name))
PLAIN_OPS = sorted(name for name in gathers.OPERATIONS if not gathers.is_compare(name))
DEPTH = (0.2, 0.4, 0.6, 0.8)                              # R of (0, 0), (1, 0), (0, 1), (1, 1)
WIDE = (0.1, 0.3, 0.5, 0.7, 0.2, 0.4, 0.6, 0.8)          # 4x2: row 0 then row 1
FIRST_ADDRESS = 10                                        # address dwords in v10…


def grid(r_values, width=2, height=2):
    """32_32_32_32_FLOAT texels: R from `r_values` (floats or raw bits), G = 1 + index, B = 0, A = 1."""
    data = bytearray()
    for index in range(width * height):
        r = r_values[index]
        r_bits = r if isinstance(r, int) else float_bits(r)
        data += struct.pack("<4I", r_bits, float_bits(1.0 + index), 0, ONE)
    return bytes(data)


def setup(texels, width=2, height=2, swizzle=(4, 5, 6, 7), **sampler):
    memory = bytearray(MEMORY_SIZE)
    memory[TEXTURE:TEXTURE + len(texels)] = texels
    t_words = images.encode_image(TEXTURE, images.FORMAT_32_32_32_32_FLOAT, width, height, swizzle)
    s_words = images.encode_sampler(**{"clamp_x": images.CLAMP_LAST_TEXEL, "clamp_y": images.CLAMP_LAST_TEXEL,
                                       **sampler})
    sgprs = {2: OUTPUT, 3: 0, **{T_SHARP + i: w for i, w in enumerate(t_words)},
             **{S_SHARP + i: w for i, w in enumerate(s_words)}}
    return memory, sgprs


def addresses(name, s, t, z=0, offset=0, lod=0):
    """v_mov of each address dword into v10… in the layout order of `name`."""
    slots = gathers.layout(name)
    values = {"offset": offset, "bias": lod, "z": z, "s": s, "t": t, "lod": lod}
    code = []
    for key in ("offset", "bias", "z", "s", "t", "lod"):
        if slots[key] is not None:
            code += [vop1("v_mov_b32", FIRST_ADDRESS + slots[key], LITERAL), values[key]]
    return code


def gather_lane0(name, memory, sgprs, s=float_bits(0.5), t=float_bits(0.5), dmask=0x1, **values):
    code = program(*addresses(name, s, t, **values), *mimg(name, 4, FIRST_ADDRESS, dmask=dmask),
                   vop1("v_mov_b32", 8, ZERO), *global_("global_store_dwordx4", addr=8, data=4, saddr=2))
    return words(run_interpreter(code, sgprs, memory), OUTPUT, 4)


def test_the_eighteen_encodings_are_executable_and_match_llvm_address_counts():
    names = {name for (family, _), name in MNEMONICS.items() if family == "MIMG" and name in gathers.OPERATIONS}
    assert names == gathers.OPERATIONS and len(names) == 18
    for name in names:
        assert rdna2_exec.executable("MIMG", name)
        assert gathers.layout(name)["count"] == address_count(name, images.DIM_2D, 0), name
    assert gathers.layout("image_gather4_c_b_cl_o") == {"offset": 0, "bias": 1, "z": 2, "s": 3, "t": 4, "lod": 5,
                                                        "count": 6}


@pytest.mark.parametrize("func, z, expected", [
    (images.COMPARE_LESS, 0.5, [ONE, ONE, 0, 0]),         # 0.5 < 0.6, 0.8 only
    (images.COMPARE_GEQUAL, 0.4, [0, 0, ONE, ONE]),
    (images.COMPARE_EQUAL, 0.8, [0, ONE, 0, 0]),
    (images.COMPARE_NOTEQUAL, 0.8, [ONE, 0, ONE, ONE]),
    (images.COMPARE_GREATER, 0.6, [0, 0, ONE, ONE]),
    (images.COMPARE_LEQUAL, 0.6, [ONE, ONE, 0, 0]),
    (images.COMPARE_NEVER, 0.5, [0, 0, 0, 0]),
    (images.COMPARE_ALWAYS, 0.5, [ONE, ONE, ONE, ONE]),
])
def test_gather4_c_returns_each_comparison_in_gather_order(func, z, expected):
    memory, sgprs = setup(grid(DEPTH), depth_compare=func)
    assert gather_lane0("image_gather4_c", memory, sgprs, z=float_bits(z)) == expected


@pytest.mark.parametrize("name", COMPARE_OPS)
def test_every_compare_form_ignores_lod_bias_and_clamp_on_a_one_level_view(name):
    memory, sgprs = setup(grid(WIDE, 4, 2), width=4, height=2, depth_compare=images.COMPARE_LESS)
    # s = 0.125 → u = 0.5 → x 0, 1; offset x = +2 moves the footprint to x 2, 3. t = 0.5 → y 0, 1.
    offset = 2 if gathers.layout(name)["offset"] is not None else 0
    got = gather_lane0(name, memory, sgprs, s=float_bits(0.125), z=float_bits(0.5), offset=offset, lod=QNAN)
    r = {(x, y): WIDE[y * 4 + x] for x in range(4) for y in range(2)}
    xs = (2, 3) if offset else (0, 1)
    footprint = [r[xs[0], 1], r[xs[1], 1], r[xs[1], 0], r[xs[0], 0]]
    assert got == [ONE if 0.5 < value else 0 for value in footprint]


@pytest.mark.parametrize("name", PLAIN_OPS)
def test_bias_and_clamp_forms_without_compare_match_image_gather4(name):
    memory, sgprs = setup(grid(DEPTH), swizzle=(5, 4, 6, 7))     # DST_SEL_X = G
    expected = gather_lane0("image_gather4", memory, sgprs, dmask=0x1)
    assert expected == [float_bits(v) for v in (3.0, 4.0, 2.0, 1.0)]   # G = 1 + texel index
    assert gather_lane0(name, memory, sgprs, dmask=0x1, lod=float_bits(-7.5)) == expected


def test_denormals_compare_as_zero_and_negative_zero_equals_zero():
    memory, sgprs = setup(grid([DENORMAL, NEG_ZERO, 0x80000005, 0.0]), depth_compare=images.COMPARE_EQUAL)
    assert gather_lane0("image_gather4_c_lz", memory, sgprs, z=0) == [ONE, ONE, ONE, ONE]
    memory, sgprs = setup(grid([DENORMAL, 0x00800000, 0.0, 0.0]), depth_compare=images.COMPARE_LESS)
    # z = denormal → 0: 0 < smallest normal (texel (1, 0), result 2) only.
    assert gather_lane0("image_gather4_c_lz", memory, sgprs, z=0x00000007) == [0, 0, ONE, 0]


@pytest.mark.parametrize("func, expected", [
    (images.COMPARE_NOTEQUAL, [ONE, ONE, ONE, ONE]), (images.COMPARE_LESS, [0, 0, 0, 0]),
    (images.COMPARE_GEQUAL, [0, 0, 0, 0]), (images.COMPARE_ALWAYS, [ONE, ONE, ONE, ONE]),
])
def test_nan_reference_is_unordered(func, expected):
    memory, sgprs = setup(grid(DEPTH), depth_compare=func)
    assert gather_lane0("image_gather4_c", memory, sgprs, z=QNAN) == expected


def test_nan_and_infinite_texels():
    memory, sgprs = setup(grid([0x7F800000, 0xFF800000, QNAN, 1.0]), depth_compare=images.COMPARE_LESS)
    # order: (0, 1) NaN, (1, 1) 1.0, (1, 0) -inf, (0, 0) +inf
    assert gather_lane0("image_gather4_c", memory, sgprs, z=float_bits(0.5)) == [0, ONE, 0, ONE]


def test_border_texels_compare_the_border_colour():
    memory, sgprs = setup(grid(DEPTH), clamp_x=images.CLAMP_BORDER, clamp_y=images.CLAMP_LAST_TEXEL,
                          border_color=images.BORDER_OPAQUE_WHITE, depth_compare=images.COMPARE_LESS)
    # s = 0.0 → x0 = -1 (border, R = 1.0), x1 = 0; t = 0.5 → y 0 and 1.
    got = gather_lane0("image_gather4_c_l", memory, sgprs, s=0, z=float_bits(0.5), lod=float_bits(3.0))
    assert got == [ONE, ONE, 0, ONE]                     # border 1.0, (0, 1) 0.6, (0, 0) 0.2, border 1.0


def test_nsa_addresses_are_read_in_layout_order():
    memory, sgprs = setup(grid(WIDE, 4, 2), width=4, height=2, depth_compare=images.COMPARE_GREATER)
    # gather4_c_lz_o: offset, z, s, t from v20, v13, v17, v11 (NSA).
    code = program(vop1("v_mov_b32", 20, LITERAL), 1, vop1("v_mov_b32", 13, LITERAL), float_bits(0.5),
                   vop1("v_mov_b32", 17, LITERAL), float_bits(0.125), vop1("v_mov_b32", 11, HALF),
                   *mimg("image_gather4_c_lz_o", 4, 0, dmask=0x1, nsa_addrs=(20, 13, 17, 11)),
                   vop1("v_mov_b32", 8, ZERO), *global_("global_store_dwordx4", addr=8, data=4, saddr=2))
    got = words(run_interpreter(code, sgprs, memory), OUTPUT, 4)
    # s = 0.125 → x 0, 1, plus offset 1 → x 1, 2; t = 0.5 → y 0, 1.
    r = {(x, y): WIDE[y * 4 + x] for x in range(4) for y in range(2)}
    assert got == [ONE if 0.5 > v else 0 for v in (r[1, 1], r[2, 1], r[2, 0], r[1, 0])]


def test_vdata_may_overlap_the_addresses():
    memory, sgprs = setup(grid(DEPTH), depth_compare=images.COMPARE_LESS)
    code = program(*addresses("image_gather4_c", float_bits(0.5), float_bits(0.5), z=float_bits(0.5)),
                   *mimg("image_gather4_c", FIRST_ADDRESS, FIRST_ADDRESS, dmask=0x1),
                   vop1("v_mov_b32", 8, ZERO), *global_("global_store_dwordx4", addr=8, data=10, saddr=2))
    assert words(run_interpreter(code, sgprs, memory), OUTPUT, 4) == [ONE, ONE, 0, 0]


@pytest.mark.parametrize("name, options, message", [
    ("image_gather4_c", {"dmask": 0x2}, "R only"),
    ("image_gather4_b", {"dmask": 0x3}, "exactly one component"),
    ("image_gather4_c_cl", {"dim": 0}, "dimension 0"),
])
def test_unsupported_instruction_fields_are_named(name, options, message):
    memory, sgprs = setup(grid(DEPTH))
    code = program(*mimg(name, 4, FIRST_ADDRESS, **{"dmask": 0x1, **options}))
    with pytest.raises(rdna2_exec.Untranslated, match=message):
        run_interpreter(code, sgprs, memory)
    with pytest.raises(Untranslatable, match=message):
        rdna2_hlsl.translate(code)


def test_unsupported_descriptors_are_named():
    memory, sgprs = setup(grid(DEPTH), swizzle=(5, 4, 6, 7))
    code = program(*mimg("image_gather4_c", 4, FIRST_ADDRESS, dmask=0x1))
    with pytest.raises(rdna2_exec.Untranslated, match="DST_SEL_X 5"):
        run_interpreter(code, sgprs, memory)
    memory, sgprs = setup(grid(DEPTH))
    with pytest.raises(rdna2_exec.Untranslated, match="TRUNC_COORD"):
        run_interpreter(code, {**sgprs, S_SHARP: sgprs[S_SHARP] | 1 << 27}, memory)
    with pytest.raises(rdna2_exec.Untranslated, match="BC_SWIZZLE 2"):
        run_interpreter(code, {**sgprs, T_SHARP + 3: sgprs[T_SHARP + 3] | 2 << 25}, memory)
    with pytest.raises(rdna2_exec.Untranslated, match="niveaux mip"):
        run_interpreter(code, {**sgprs, T_SHARP + 3: sgprs[T_SHARP + 3] | 1 << 16}, memory)


def test_still_refused_neighbours_keep_their_names():
    memory, sgprs = setup(grid(DEPTH))
    for name in ("image_sample_b", "image_gather4h", "image_get_lod"):
        with pytest.raises(rdna2_exec.Untranslated, match=name):
            run_interpreter(program(*mimg(name, 4, FIRST_ADDRESS)), sgprs, memory)
