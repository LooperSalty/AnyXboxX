"""image_gather4 with compare, bias or clamp recompiled to HLSL: the same bytes as the reference interpreter.

Each program runs the 18 gathers of emu/rdna2_exec_gather.py over lane-dependent coordinates, offsets,
z references (special values included) and lod/bias/clamp dwords, through 8 samplers (the 8 compare
functions, each with other addressing modes and border colours), in wave32 and wave64, with all lanes or
a partial EXEC (inactive lanes keep their prefilled results). Values computed by hand:
tests/test_rdna2_exec_gather.py.
"""
import random
import struct

import pytest

from emu import rdna2_exec, rdna2_exec_gather as gathers, rdna2_gpu, rdna2_hlsl, rdna2_image as images
from emu.rdna2_exec_ops import float_bits
from test_rdna2_exec import const, program, sop1, vop1, vop2
from test_rdna2_exec_memory import INPUT, OUTPUT, global_
from test_rdna2_image import LITERAL, MEMORY_SIZE, TEXTURE, mimg

gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")
EXEC_LO, EXEC_HI, MINUS_ONE = 126, 127, 193
T_DEPTH, T_COLOR = 8, 48                                  # s[8:15] 32_32_32_32_FLOAT, s[48:55] 8_8_8_8_UNORM
SAMPLERS = tuple(16 + 4 * k for k in range(8))            # s[16:47]: compare function k
COLOR_TEXTURE = TEXTURE + 0x1000
WIDTH, HEIGHT = 8, 4
S, T, Z, OFF, LOD, LANE16 = 1, 2, 3, 4, 5, 30             # address sources and lane × 16
DATA = 20                                                 # results in v20-v23
SPECIAL = [0x00000000, 0x80000000, 0x00000001, 0x807FFFFF, 0x00800000, 0x7F800000, 0xFF800000, 0x7FC00000,
           0xFFC00001, 0x3F000000, 0x3E800000, 0x3F400000, 0x3F800000, 0xBF800000]
CLAMPS = (images.CLAMP_WRAP, images.CLAMP_MIRROR, images.CLAMP_LAST_TEXEL, images.CLAMP_MIRROR_ONCE_LAST_TEXEL,
          images.CLAMP_BORDER, images.CLAMP_MIRROR_ONCE_BORDER, images.CLAMP_BORDER, images.CLAMP_LAST_TEXEL)
BORDERS = (images.BORDER_TRANS_BLACK, images.BORDER_OPAQUE_BLACK, images.BORDER_OPAQUE_WHITE)


def depth_texels(seed):
    """8x4 32_32_32_32_FLOAT texels: R mixes special values and quarter steps, G/B/A random bits."""
    generator = random.Random(seed)
    data = bytearray()
    for index in range(WIDTH * HEIGHT):
        r = SPECIAL[index] if index < len(SPECIAL) else float_bits(generator.randrange(8) / 4)
        data += struct.pack("<4I", r, *(generator.getrandbits(32) for _ in range(3)))
    return bytes(data)


def z_inputs(lanes, seed):
    """One z reference per lane: special values, then quarter steps the texels also hold."""
    generator = random.Random(seed)
    values = [SPECIAL[lane] if lane < len(SPECIAL) else float_bits(generator.randrange(-1, 9) / 4)
              for lane in range(lanes)]
    generator.shuffle(values)
    return struct.pack(f"<{lanes}I", *values)


def state(lanes, seed):
    memory = bytearray(MEMORY_SIZE)
    texels = depth_texels(seed)
    memory[TEXTURE:TEXTURE + len(texels)] = texels
    generator = random.Random(seed + 1)
    memory[COLOR_TEXTURE:COLOR_TEXTURE + WIDTH * HEIGHT * 4] = bytes(generator.getrandbits(8)
                                                                     for _ in range(WIDTH * HEIGHT * 4))
    memory[INPUT:INPUT + lanes * 4] = z_inputs(lanes, seed)
    sgprs = {0: INPUT, 1: 0, 2: OUTPUT, 3: 0}
    for base, address, fmt, swizzle in ((T_DEPTH, TEXTURE, images.FORMAT_32_32_32_32_FLOAT, (4, 6, 5, 1)),
                                        (T_COLOR, COLOR_TEXTURE, images.FORMAT_8_8_8_8_UNORM, (6, 5, 4, 7))):
        sgprs.update({base + i: w for i, w in enumerate(images.encode_image(address, fmt, WIDTH, HEIGHT, swizzle))})
    for func, base in enumerate(SAMPLERS):
        clamp = CLAMPS[func]
        sampler = images.encode_sampler(clamp_x=clamp, clamp_y=CLAMPS[(func + 3) % 8], unnormalized=func == 5,
                                        depth_compare=func, border_color=BORDERS[func % 3])
        sgprs.update({base + i: w for i, w in enumerate(sampler)})
    return bytes(memory), sgprs


def lane_setup():
    """v1 = s, v2 = t (lane-dependent, outside [0, 1] too), v3 = z (from INPUT), v4 = offsets, v5 = lod bits."""
    return [vop2("v_lshlrev_b32", 31, const(2), 0),
            *global_("global_load_dword", addr=31, vdst=Z, saddr=0),
            vop1("v_cvt_f32_u32", S, 256),
            vop2("v_mul_f32", S, LITERAL, S), float_bits(0.046875),
            vop2("v_add_f32", S, LITERAL, S), float_bits(-0.3),
            vop2("v_and_b32", T, const(7), 0),
            vop1("v_cvt_f32_u32", T, 256 + T),
            vop2("v_mul_f32", T, LITERAL, T), float_bits(0.21875),
            vop2("v_add_f32", T, LITERAL, T), float_bits(-0.4),
            vop2("v_mul_u32_u24", OFF, LITERAL, 0), 0x0B2D,
            vop2("v_mul_u32_u24", LOD, LITERAL, 0), 0x01234567 & 0xFFFFFF,
            vop2("v_lshlrev_b32", LANE16, const(4), 0)]


def address_block(name, first=10):
    """Copies the sources into v10… in the layout order of `name` (consecutive addresses)."""
    slots = gathers.layout(name)
    sources = {"offset": OFF, "bias": LOD, "z": Z, "s": S, "t": T, "lod": LOD}
    return [vop1("v_mov_b32", first + slots[key], 256 + sources[key])
            for key in ("offset", "bias", "z", "s", "t", "lod") if slots[key] is not None]


def nsa(name):
    slots = gathers.layout(name)
    sources = {"offset": OFF, "bias": LOD, "z": Z, "s": S, "t": T, "lod": LOD}
    return tuple(sources[key] for key in ("offset", "bias", "z", "s", "t", "lod") if slots[key] is not None)


def gathers_program(lanes, exec_mask=None):
    """Every gather once, each with its own sampler and output slot; optional partial EXEC per gather."""
    words = lane_setup()
    slot_bytes = lanes * 16
    for slot, name in enumerate(sorted(gathers.OPERATIONS)):
        compare = gathers.is_compare(name)
        rsrc = T_DEPTH if compare or slot % 2 else T_COLOR
        dmask = 0x1 if compare else 1 << (slot % 4)
        words += [word for c in range(4) for word in (vop2("v_mul_u32_u24", DATA + c, LITERAL, 0), 0x10101 * (slot + 1) + c)]
        if exec_mask is not None:
            words += [sop1("s_mov_b32", EXEC_LO, LITERAL), exec_mask & 0xFFFFFFFF]
            if lanes == 64:
                words += [sop1("s_mov_b32", EXEC_HI, LITERAL), exec_mask >> 32]
        if len(nsa(name)) <= 5 and slot % 3:
            words += mimg(name, DATA, 0, srsrc=rsrc, ssamp=SAMPLERS[slot % 8], dmask=dmask, unorm=slot % 5 == 4,
                          nsa_addrs=nsa(name))
        else:
            words += address_block(name)
            words += mimg(name, DATA, 10, srsrc=rsrc, ssamp=SAMPLERS[slot % 8], dmask=dmask)
        if exec_mask is not None:
            words += [sop1("s_mov_b32", EXEC_LO, MINUS_ONE)]
            if lanes == 64:
                words += [sop1("s_mov_b32", EXEC_HI, MINUS_ONE)]
        words += [vop2("v_add_nc_u32", 32, LITERAL, LANE16), slot * slot_bytes,
                  *global_("global_store_dwordx4", addr=32, data=DATA, saddr=2)]
    return program(*words)


def run_both(code, lanes, memory, sgprs):
    wave = rdna2_exec.Wave(lanes=lanes)
    wave.memory.write(0, memory)
    for number, value in sgprs.items():
        wave.s[number] = value
    rdna2_exec.run(code, wave)
    expected = wave.memory.read(0, MEMORY_SIZE)
    actual = rdna2_gpu.run(rdna2_hlsl.translate(code, sgprs, wave_size=lanes), memory)
    return expected, actual[:MEMORY_SIZE]


def test_programs_cover_the_eighteen_gathers_and_every_compare_function():
    assert len(gathers.OPERATIONS) == 18
    code = gathers_program(32)
    hlsl = rdna2_hlsl.translate(code, state(32, 1)[1])
    assert hlsl.count("GatherCompare(t, q") == 12 and hlsl.count("ImageGather4(t, q") == 6


@gpu
@pytest.mark.parametrize("lanes, exec_mask, seed", [
    (32, None, 1),
    (32, 0xB5E3_0F71, 2),
    (64, None, 3),
    (64, 0x8F0F_00F3_7E1C_A55A, 4),
])
def test_gathers_match_the_interpreter(lanes, exec_mask, seed):
    memory, sgprs = state(lanes, seed)
    expected, actual = run_both(gathers_program(lanes, exec_mask), lanes, memory, sgprs)
    assert actual == expected
    results = expected[OUTPUT:OUTPUT + 18 * lanes * 16]
    assert results.count(struct.pack("<I", images.ONE_BITS)) > 0 and any(results)
