"""Tests des 16 instructions MIMG image_atomic_* (32 bits et 64 bits) dans l'interpréteur et en HLSL.

Vérifie la sémantique RDNA 2 (dmask, glc, hors limites, NSA, chevauchement vdata/vaddr, contention
multi-lignes, cas limites signés/flottants/NaN/±0), les formats entiers 32/64 bits, la traduction
HLSL (wave32 et wave64) et la parité bit-à-bit sur GPU (@gpu).
"""
import struct

import pytest

from emu import rdna2_exec, rdna2_exec_image, rdna2_gpu, rdna2_hlsl, rdna2_hlsl_graphics, rdna2_image as images
from emu.rdna2_exec_ops import float_bits
from emu.rdna2_hlsl_ops import Untranslatable
from test_rdna2_exec import EXEC_LO, const, program, sop1, vop1, vop2
from test_rdna2_exec_memory import OUTPUT, global_
from test_rdna2_graphics import words
from test_rdna2_image import LITERAL, MEMORY_SIZE, OPCODES, T_SHARP, TEXTURE, gpu, run_interpreter

ZERO, ONE_INT = 128, 129
NAN32 = 0x7FC0_0000
NAN64 = 0x7FF8_0000_0000_0000
NEG_ZERO32 = 0x8000_0000
NEG_ZERO64 = 0x8000_0000_0000_0000


def double_bits(value):
    return struct.unpack("<Q", struct.pack("<d", value))[0]


def mimg_atomic(name, vdata, vaddr, srsrc=T_SHARP, dmask=0x1, glc=1, dim=images.DIM_2D, nsa_addrs=None, extra_low=0):
    op = OPCODES["MIMG", name]
    nsa = 1 if nsa_addrs else 0
    low = (0x3C << 26 | (op & 0x7F) << 18 | glc << 13 | dmask << 8
           | dim << 3 | nsa << 1 | (op >> 7) & 1 | extra_low)
    first_addr = nsa_addrs[0] if nsa_addrs else vaddr
    high = (srsrc // 4) << 16 | vdata << 8 | first_addr
    if not nsa_addrs:
        return low, high
    extra = sum((reg & 0xFF) << (8 * i) for i, reg in enumerate(nsa_addrs[1:]))
    return low, high, extra


def _pack_tsharp(base_sgpr, address, fmt, width, height, swizzle=(4, 5, 6, 7)):
    return {base_sgpr + i: w for i, w in enumerate(images.encode_image(address, fmt, width, height, swizzle))}


def _setup_image(fmt, width, height, raw_bytes):
    memory = bytearray(MEMORY_SIZE)
    memory[TEXTURE:TEXTURE + len(raw_bytes)] = raw_bytes
    sgprs = {2: OUTPUT, 3: 0, **_pack_tsharp(T_SHARP, TEXTURE, fmt, width, height)}
    return memory, sgprs


def _run_single_atomic32(op_name, fmt, init_word, data0, data1=0, glc=1):
    memory, sgprs = _setup_image(fmt, 4, 2, struct.pack("<I", init_word))
    is_cmp = op_name in images.ATOMIC_CMPSWAP_OPS
    dmask = 0x3 if is_cmp else 0x1
    insts = [
        sop1("s_mov_b32", EXEC_LO, ONE_INT),                              # seule la ligne 0 est active
        vop1("v_mov_b32", 1, ZERO),                                       # x = 0
        vop1("v_mov_b32", 2, ZERO),                                       # y = 0
        vop1("v_mov_b32", 4, LITERAL), data0,                             # vdata[0]
        vop1("v_mov_b32", 5, LITERAL), data1,                             # vdata[1] (cmp ou sentinelle)
        *mimg_atomic(op_name, 4, 1, dmask=dmask, glc=glc),
        vop1("v_mov_b32", 10, ZERO),
        *global_("global_store_dwordx2", addr=10, data=4, saddr=2),
    ]
    final = run_interpreter(program(*insts), sgprs, memory)
    mem_word = words(final, TEXTURE, 1)[0]
    ret_v4, ret_v5 = words(final, OUTPUT, 2)
    return mem_word, ret_v4, ret_v5


def test_coverage_and_encoding_of_all_16_image_atomics():
    expected = {
        "image_atomic_swap", "image_atomic_cmpswap", "image_atomic_add", "image_atomic_sub",
        "image_atomic_smin", "image_atomic_umin", "image_atomic_smax", "image_atomic_umax",
        "image_atomic_and", "image_atomic_or", "image_atomic_xor", "image_atomic_inc",
        "image_atomic_dec", "image_atomic_fcmpswap", "image_atomic_fmin", "image_atomic_fmax",
    }
    assert images.ATOMIC_OPS == expected
    for name in expected:
        assert rdna2_exec_image.supports("MIMG", name)
        assert images.address_count(name) == 2
        dmask = 0x3 if name in images.ATOMIC_CMPSWAP_OPS else 0x1
        for glc in (0, 1):
            low, high = mimg_atomic(name, 4, 1, dmask=dmask, glc=glc)
            fields = images.instruction_fields(low | high << 32)
            assert fields["glc"] == glc and fields["dmask"] == dmask
            assert images.atomic_width(name, dmask) == 32


def test_all_16_image_atomics_32bit_glc_and_no_glc():
    cases = [
        ("image_atomic_swap", images.FORMAT_32_UINT, 0x1111_2222, 0xDEAD_BEEF, 0, 0xDEAD_BEEF),
        ("image_atomic_cmpswap", images.FORMAT_32_UINT, 42, 99, 42, 99),
        ("image_atomic_cmpswap", images.FORMAT_32_UINT, 42, 99, 43, 42),
        ("image_atomic_add", images.FORMAT_32_UINT, 0xFFFF_FFFF, 2, 0, 1),
        ("image_atomic_sub", images.FORMAT_32_UINT, 1, 3, 0, 0xFFFF_FFFE),
        ("image_atomic_smin", images.FORMAT_32_SINT, 3, 0xFFFF_FFFB, 0, 0xFFFF_FFFB),        # min(3, -5) = -5
        ("image_atomic_smin", images.FORMAT_32_SINT, 0xFFFF_FFF6, 0xFFFF_FFFE, 0, 0xFFFF_FFF6),  # min(-10, -2) = -10
        ("image_atomic_umin", images.FORMAT_32_UINT, 0xFFFF_FFFF, 3, 0, 3),
        ("image_atomic_smax", images.FORMAT_32_SINT, 0xFFFF_FFFB, 3, 0, 3),                  # max(-5, 3) = 3
        ("image_atomic_smax", images.FORMAT_32_SINT, 0xFFFF_FFF6, 0xFFFF_FFFE, 0, 0xFFFF_FFFE),  # max(-10, -2) = -2
        ("image_atomic_umax", images.FORMAT_32_UINT, 3, 0xFFFF_FFFF, 0, 0xFFFF_FFFF),
        ("image_atomic_and", images.FORMAT_32_UINT, 0xF0F0_0F0F, 0x0FF0_FF00, 0, 0x00F0_0F00),
        ("image_atomic_or", images.FORMAT_32_UINT, 0xF000_000F, 0x00F0_0F00, 0, 0xF0F0_0F0F),
        ("image_atomic_xor", images.FORMAT_32_UINT, 0xAAAA_5555, 0xFFFF_0000, 0, 0x5555_5555),
        ("image_atomic_inc", images.FORMAT_32_UINT, 4, 5, 0, 5),
        ("image_atomic_inc", images.FORMAT_32_UINT, 5, 5, 0, 0),                             # wrap à >= limite
        ("image_atomic_dec", images.FORMAT_32_UINT, 3, 7, 0, 2),
        ("image_atomic_dec", images.FORMAT_32_UINT, 0, 7, 0, 7),                             # wrap à 0
        ("image_atomic_dec", images.FORMAT_32_UINT, 9, 7, 0, 7),                             # wrap à > limite
        ("image_atomic_fcmpswap", images.FORMAT_32_FLOAT, NEG_ZERO32, float_bits(7.5), 0, float_bits(7.5)),
        ("image_atomic_fcmpswap", images.FORMAT_32_FLOAT, NAN32, float_bits(7.5), NAN32, NAN32),
        ("image_atomic_fcmpswap", images.FORMAT_32_FLOAT, float_bits(2.0), float_bits(5.0), float_bits(3.0), float_bits(2.0)),
        ("image_atomic_fmin", images.FORMAT_32_FLOAT, float_bits(2.5), float_bits(-1.25), 0, float_bits(-1.25)),
        ("image_atomic_fmin", images.FORMAT_32_FLOAT, NAN32, float_bits(1.0), 0, float_bits(1.0)),
        ("image_atomic_fmin", images.FORMAT_32_FLOAT, float_bits(1.0), NAN32, 0, float_bits(1.0)),
        ("image_atomic_fmax", images.FORMAT_32_FLOAT, float_bits(-2.5), float_bits(1.25), 0, float_bits(1.25)),
        ("image_atomic_fmax", images.FORMAT_32_FLOAT, NAN32, float_bits(-3.0), 0, float_bits(-3.0)),
        ("image_atomic_fmax", images.FORMAT_32_FLOAT, float_bits(-3.0), NAN32, 0, float_bits(-3.0)),
    ]
    for op_name, fmt, init_val, data0, data1, expected_mem in cases:
        mem_glc, ret_v4, ret_v5 = _run_single_atomic32(op_name, fmt, init_val, data0, data1, glc=1)
        assert mem_glc == expected_mem, f"{op_name} glc=1 mem"
        assert ret_v4 == init_val, f"{op_name} glc=1 old"
        assert ret_v5 == data1, f"{op_name} glc=1 v5 préservé"
        mem_noglc, kept_v4, kept_v5 = _run_single_atomic32(op_name, fmt, init_val, data0, data1, glc=0)
        assert mem_noglc == expected_mem, f"{op_name} glc=0 mem"
        assert kept_v4 == data0 and kept_v5 == data1, f"{op_name} glc=0 vdata inchangé"


def test_image_atomics_64bit_interpreter():
    cases = [
        ("image_atomic_swap", images.FORMAT_32_32_UINT, 0x0102_0304_0506_0708, 0xF0E0_D0C0_B0A0_9080, 0, 0xF0E0_D0C0_B0A0_9080),
        ("image_atomic_add", images.FORMAT_32_32_UINT, 0x0000_0001_FFFF_FFFF, 2, 0, 0x0000_0002_0000_0001),
        ("image_atomic_sub", images.FORMAT_32_32_UINT, 0x0000_0002_0000_0000, 1, 0, 0x0000_0001_FFFF_FFFF),
        ("image_atomic_smin", images.FORMAT_32_32_SINT, 1, (1 << 64) - 5, 0, (1 << 64) - 5),
        ("image_atomic_smax", images.FORMAT_32_32_SINT, (1 << 64) - 5, 1, 0, 1),
        ("image_atomic_umin", images.FORMAT_32_32_UINT, (1 << 64) - 1, 5, 0, 5),
        ("image_atomic_umax", images.FORMAT_32_32_UINT, 5, (1 << 64) - 1, 0, (1 << 64) - 1),
        ("image_atomic_and", images.FORMAT_32_32_UINT, 0xFFFF_0000_F0F0_0F0F, 0x00FF_FFFF_0FF0_FF00, 0, 0x00FF_0000_00F0_0F00),
        ("image_atomic_or", images.FORMAT_32_32_UINT, 0x0000_0001_0000_0000, 0x0000_0000_0000_0002, 0, 0x0000_0001_0000_0002),
        ("image_atomic_xor", images.FORMAT_32_32_UINT, 0x1234_5678_9ABC_DEF0, 0xFFFF_FFFF_0000_0000, 0, 0xEDCB_A987_9ABC_DEF0),
        ("image_atomic_inc", images.FORMAT_32_32_UINT, 1 << 32, 1 << 33, 0, (1 << 32) + 1),
        ("image_atomic_dec", images.FORMAT_32_32_UINT, 0, 1 << 33, 0, 1 << 33),
        ("image_atomic_cmpswap", images.FORMAT_32_32_UINT, 0x1111_2222_3333_4444, 0xAAAA_BBBB_CCCC_DDDD,
         0x1111_2222_3333_4444, 0xAAAA_BBBB_CCCC_DDDD),
        ("image_atomic_cmpswap", images.FORMAT_32_32_UINT, 0x1111_2222_3333_4444, 0xAAAA_BBBB_CCCC_DDDD,
         0x1111_2222_3333_4445, 0x1111_2222_3333_4444),
        ("image_atomic_fcmpswap", images.FORMAT_32_32_FLOAT, NEG_ZERO64, double_bits(3.5), 0, double_bits(3.5)),
        ("image_atomic_fcmpswap", images.FORMAT_32_32_FLOAT, NAN64, double_bits(3.5), NAN64, NAN64),
        ("image_atomic_fmin", images.FORMAT_32_32_FLOAT, double_bits(2.0), double_bits(-1.5), 0, double_bits(-1.5)),
        ("image_atomic_fmax", images.FORMAT_32_32_FLOAT, NAN64, double_bits(4.25), 0, double_bits(4.25)),
    ]
    for op_name, fmt, init_val, data0, data1, expected_mem in cases:
        memory, sgprs = _setup_image(fmt, 2, 2, struct.pack("<Q", init_val))
        dmask = 0xF if op_name in images.ATOMIC_CMPSWAP_OPS else 0x3
        insts = [
            sop1("s_mov_b32", EXEC_LO, ONE_INT),
            vop1("v_mov_b32", 1, ZERO),
            vop1("v_mov_b32", 2, ZERO),
            vop1("v_mov_b32", 4, LITERAL), data0 & 0xFFFFFFFF,
            vop1("v_mov_b32", 5, LITERAL), (data0 >> 32) & 0xFFFFFFFF,
            vop1("v_mov_b32", 6, LITERAL), data1 & 0xFFFFFFFF,
            vop1("v_mov_b32", 7, LITERAL), (data1 >> 32) & 0xFFFFFFFF,
            *mimg_atomic(op_name, 4, 1, dmask=dmask, glc=1),
            vop1("v_mov_b32", 10, ZERO),
            *global_("global_store_dwordx4", addr=10, data=4, saddr=2),
        ]
        final = run_interpreter(program(*insts), sgprs, memory)
        got_mem = struct.unpack_from("<Q", final, TEXTURE)[0]
        got_ret = struct.unpack_from("<Q", final, OUTPUT)[0]
        got_cmp = struct.unpack_from("<Q", final, OUTPUT + 8)[0]
        assert got_mem == expected_mem, f"{op_name} 64-bit mem"
        assert got_ret == init_val, f"{op_name} 64-bit old"
        assert got_cmp == data1, f"{op_name} 64-bit cmp préservé"


def test_image_atomics_oob_nsa_overlap_and_multi_lane_contention():
    # 1) Contention multi-lignes : les 32 lignes ajoutent 1 au texel (1, 1) d'une image 4×4
    grid = struct.pack("<16I", *[100 + i for i in range(16)])
    memory, sgprs = _setup_image(images.FORMAT_32_UINT, 4, 4, grid)
    code_contend = program(
        vop1("v_mov_b32", 10, ONE_INT),                                   # x = 1 (NSA)
        vop1("v_mov_b32", 14, ONE_INT),                                   # y = 1 (NSA)
        vop1("v_mov_b32", 4, ONE_INT),                                    # +1
        *mimg_atomic("image_atomic_add", 4, 0, dmask=0x1, glc=1, nsa_addrs=(10, 14)),
        vop2("v_lshlrev_b32", 8, const(2), 0),
        *global_("global_store_dword", addr=8, data=4, saddr=2),
    )
    final = run_interpreter(code_contend, sgprs, memory)
    assert words(final, TEXTURE + (1 * 4 + 1) * 4, 1) == [105 + 32]
    assert words(final, OUTPUT, 32) == [105 + lane for lane in range(32)]

    # 2) Hors limites (x=4, y=0 et x=-1, y=0) + chevauchement vdata == vaddr + lignes inactives
    memory2, sgprs2 = _setup_image(images.FORMAT_32_UINT, 4, 2, struct.pack("<8I", *[50 + i for i in range(8)]))
    code_edge = program(
        # Lignes 0..3 : x = lane (dans l'image) ; lignes 4..7 : x = lane (hors limites >= 4) ; lignes 8..31 inactives
        sop1("s_mov_b32", EXEC_LO, LITERAL), 0xFF,
        vop1("v_mov_b32", 1, 256),                                        # v1 = lane (x ET vdata !)
        vop1("v_mov_b32", 2, ZERO),                                       # v2 = 0 (y)
        *mimg_atomic("image_atomic_add", 1, 1, dmask=0x1, glc=1),         # chevauchement vdata=v1, vaddr=v[1:2]
        sop1("s_mov_b32", EXEC_LO, LITERAL), 0xFFFF_FFFF,
        vop2("v_lshlrev_b32", 8, const(2), 0),
        *global_("global_store_dword", addr=8, data=1, saddr=2),
    )
    final2 = run_interpreter(code_edge, sgprs2, memory2)
    assert words(final2, TEXTURE, 4) == [50, 52, 54, 56]                  # 50+0, 51+1, 52+2, 53+3
    assert words(final2, TEXTURE + 16, 4) == [54, 55, 56, 57]             # ligne y=1 intacte
    assert words(final2, OUTPUT, 8) == [50, 51, 52, 53, 0, 0, 0, 0]       # OOB avec glc=1 renvoie 0
    assert words(final2, OUTPUT + 32, 4) == [0, 0, 0, 0]                  # lignes inactives non modifiées (v1=0)

    # 3) Hors limites 64 bits avec glc=1 renvoie 0 sur les deux mots sans toucher la mémoire
    memory3, sgprs3 = _setup_image(images.FORMAT_32_32_UINT, 2, 2, struct.pack("<4Q", 11, 22, 33, 44))
    code_oob64 = program(
        sop1("s_mov_b32", EXEC_LO, ONE_INT),
        vop1("v_mov_b32", 1, LITERAL), 0xFFFF_FFFF,                       # x = -1 (OOB)
        vop1("v_mov_b32", 2, ZERO),
        vop1("v_mov_b32", 4, LITERAL), 0xAAAA_BBBB,
        vop1("v_mov_b32", 5, LITERAL), 0xCCCC_DDDD,
        *mimg_atomic("image_atomic_swap", 4, 1, dmask=0x3, glc=1),
        vop1("v_mov_b32", 8, ZERO),
        *global_("global_store_dwordx2", addr=8, data=4, saddr=2),
    )
    final3 = run_interpreter(code_oob64, sgprs3, memory3)
    assert struct.unpack_from("<4Q", final3, TEXTURE) == (11, 22, 33, 44)
    assert words(final3, OUTPUT, 2) == [0, 0]


def test_integer_formats_load_and_store_round_trip():
    dst_addr, t_dst = 0x70000, 20
    for fmt, raw in [
        (images.FORMAT_32_UINT, struct.pack("<8I", *[0x1000_0000 + i for i in range(8)])),
        (images.FORMAT_32_SINT, struct.pack("<8i", *[-4 + i for i in range(8)])),
        (images.FORMAT_32_32_UINT, struct.pack("<16I", *[i * 17 + 3 for i in range(16)])),
        (images.FORMAT_32_32_SINT, struct.pack("<16i", *[-8 + i for i in range(16)])),
    ]:
        memory, sgprs = _setup_image(fmt, 4, 2, raw)
        sgprs.update(_pack_tsharp(t_dst, dst_addr, fmt, 4, 2))
        code = program(
            vop2("v_and_b32", 1, const(3), 0),
            vop2("v_lshrrev_b32", 2, const(2), 0),
            *mimg_atomic("image_load", 4, 1, srsrc=T_SHARP, dmask=0xF, glc=0),
            *mimg_atomic("image_store", 4, 1, srsrc=t_dst, dmask=0xF, glc=0),
            vop2("v_lshlrev_b32", 10, const(4), 0),
            *global_("global_store_dwordx4", addr=10, data=4, saddr=2),
        )
        final = run_interpreter(code, sgprs, memory)
        assert final[dst_addr:dst_addr + len(raw)] == raw
        assert words(final, OUTPUT, 4)[3] == 1                            # alpha entier = 1


def test_image_atomics_refusals_interpreter_and_hlsl():
    memory, sgprs = _setup_image(images.FORMAT_32_UINT, 4, 4, bytes(64))
    for op_name, bad_dmask in [("image_atomic_add", 0x0), ("image_atomic_add", 0x5),
                               ("image_atomic_cmpswap", 0x1), ("image_atomic_fcmpswap", 0x7)]:
        bad = program(*mimg_atomic(op_name, 4, 1, dmask=bad_dmask, glc=1))
        with pytest.raises(rdna2_exec.Untranslated, match="dmask"):
            run_interpreter(bad, sgprs, memory)
        with pytest.raises(Untranslatable, match="dmask"):
            rdna2_hlsl.translate(bad)

    # Format incompatible avec la largeur de l'atomique
    bad_fmt_sgprs = {2: OUTPUT, 3: 0, **_pack_tsharp(T_SHARP, TEXTURE, images.FORMAT_8_8_8_8_UNORM, 4, 4)}
    with pytest.raises(rdna2_exec.Untranslated, match="GFX10_FORMAT_8_8_8_8_UNORM"):
        run_interpreter(program(*mimg_atomic("image_atomic_add", 4, 1, dmask=0x1, glc=1)), bad_fmt_sgprs, memory)
    with pytest.raises(rdna2_exec.Untranslated, match="GFX10_FORMAT_32_UINT"):
        run_interpreter(program(*mimg_atomic("image_atomic_add", 4, 1, dmask=0x3, glc=1)), sgprs, memory)

    # 64 bits refusé proprement en HLSL
    for op_name, dmask in [("image_atomic_add", 0x3), ("image_atomic_cmpswap", 0xF)]:
        with pytest.raises(Untranslatable, match="atomique 64 bits"):
            rdna2_hlsl.translate(program(*mimg_atomic(op_name, 4, 1, dmask=dmask, glc=1)))

    # Dimension != 2D et option tfe refusées
    one_d = program(*mimg_atomic("image_atomic_swap", 4, 1, dmask=0x1, dim=0))
    tfe = program(*mimg_atomic("image_atomic_swap", 4, 1, dmask=0x1, extra_low=1 << 16))
    for bad, pattern in ((one_d, "dimension 0"), (tfe, "option tfe")):
        with pytest.raises(rdna2_exec.Untranslated, match=pattern):
            run_interpreter(bad, sgprs, memory)
        with pytest.raises(Untranslatable, match=pattern):
            rdna2_hlsl.translate(bad)


def test_hlsl_translation_all_16_atomics_wave32_wave64_and_graphics():
    ordered = [
        "image_atomic_swap", "image_atomic_cmpswap", "image_atomic_add", "image_atomic_sub",
        "image_atomic_smin", "image_atomic_umin", "image_atomic_smax", "image_atomic_umax",
        "image_atomic_and", "image_atomic_or", "image_atomic_xor", "image_atomic_inc",
        "image_atomic_dec", "image_atomic_fcmpswap", "image_atomic_fmin", "image_atomic_fmax",
    ]
    insts = []
    for idx, name in enumerate(ordered):
        dmask = 0x3 if name in images.ATOMIC_CMPSWAP_OPS else 0x1
        glc = idx & 1
        nsa = (10, 12) if idx == 2 else None
        insts.extend(mimg_atomic(name, 4, 1, dmask=dmask, glc=glc, nsa_addrs=nsa))
    code = program(*insts)
    for wave_size in (32, 64):
        hlsl = rdna2_hlsl.translate(code, wave_size=wave_size)
        assert "uint ImageAtomic32(" in hlsl
        assert "AllMemoryBarrierWithGroupSync();" in hlsl
        for op_code in range(16):
            assert f", {op_code}u, " in hlsl
        assert "ImageAtomic32(t, v[10], v[12], 2u, v[4], 0u);" in hlsl
        assert "uint old = ImageAtomic32(t, v[1], v[2], 1u, v[4], v[5]); v[4] = old;" in hlsl
    # Helper présent dans les étages graphiques sans casser la traduction
    assert "uint ImageAtomic32(" in rdna2_hlsl_graphics.translate_pixel(program(), 8)


@gpu
def test_gpu_image_atomics_32bit_match_interpreter():
    # Chaque ligne travaille sur son propre texel (lane & 7, lane >> 3) : résultat déterministe bit-à-bit.
    ops_u32 = [
        ("image_atomic_swap", 0x1, 0x1234_5678, 0),
        ("image_atomic_cmpswap", 0x3, 0xCAFE_BABE, 10),
        ("image_atomic_add", 0x1, 7, 0),
        ("image_atomic_sub", 0x1, 15, 0),
        ("image_atomic_umin", 0x1, 12, 0),
        ("image_atomic_umax", 0x1, 20, 0),
        ("image_atomic_and", 0x1, 0xF, 0),
        ("image_atomic_or", 0x1, 0xF00, 0),
        ("image_atomic_xor", 0x1, 0xFF, 0),
        ("image_atomic_inc", 0x1, 10, 0),
        ("image_atomic_dec", 0x1, 10, 0),
    ]
    raw_u32 = struct.pack("<32I", *[lane for lane in range(32)])
    memory, sgprs = _setup_image(images.FORMAT_32_UINT, 8, 4, raw_u32)
    insts = [
        vop2("v_and_b32", 1, const(7), 0),
        vop2("v_lshrrev_b32", 2, const(3), 0),
    ]
    for idx, (name, dmask, d0, d1) in enumerate(ops_u32):
        insts += [
            vop1("v_mov_b32", 4, LITERAL), d0,
            vop1("v_mov_b32", 5, LITERAL), d1,
            *mimg_atomic(name, 4, 1, dmask=dmask, glc=idx & 1),
        ]
    insts += [
        vop2("v_lshlrev_b32", 8, const(2), 0),
        *global_("global_store_dword", addr=8, data=4, saddr=2),
    ]
    code = program(*insts)
    expected = run_interpreter(code, sgprs, memory)
    actual = rdna2_gpu.run(rdna2_hlsl.translate(code, sgprs), bytes(memory))
    assert actual[:MEMORY_SIZE] == expected

    # smin/smax + flottants (fcmpswap, fmin, fmax avec NaN et -0.0)
    raw_f32 = struct.pack("<32I", *[
        NEG_ZERO32 if lane == 0 else NAN32 if lane == 1 else float_bits(lane - 16.0)
        for lane in range(32)
    ])
    mem_f, sgprs_f = _setup_image(images.FORMAT_32_FLOAT, 8, 4, raw_f32)
    code_f = program(
        vop2("v_and_b32", 1, const(7), 0),
        vop2("v_lshrrev_b32", 2, const(3), 0),
        vop1("v_mov_b32", 4, LITERAL), float_bits(9.5),
        vop1("v_mov_b32", 5, ZERO),
        *mimg_atomic("image_atomic_fcmpswap", 4, 1, dmask=0x3, glc=1),
        vop1("v_mov_b32", 6, LITERAL), float_bits(1.5),
        *mimg_atomic("image_atomic_fmin", 6, 1, dmask=0x1, glc=1),
        vop1("v_mov_b32", 7, LITERAL), float_bits(-2.5),
        *mimg_atomic("image_atomic_fmax", 7, 1, dmask=0x1, glc=1),
        vop1("v_mov_b32", 8, LITERAL), 0xFFFF_FFFB,
        *mimg_atomic("image_atomic_smin", 8, 1, dmask=0x1, glc=1),
        vop1("v_mov_b32", 9, LITERAL), 5,
        *mimg_atomic("image_atomic_smax", 9, 1, dmask=0x1, glc=1),
        vop2("v_lshlrev_b32", 12, const(2), 0),
        *global_("global_store_dword", addr=12, data=4, saddr=2),
    )
    expected_f = run_interpreter(code_f, sgprs_f, mem_f)
    actual_f = rdna2_gpu.run(rdna2_hlsl.translate(code_f, sgprs_f), bytes(mem_f))
    assert actual_f[:MEMORY_SIZE] == expected_f


@gpu
def test_gpu_image_atomic_contention_matches_interpreter():
    memory, sgprs = _setup_image(images.FORMAT_32_UINT, 4, 4, struct.pack("<16I", *([10] * 16)))
    code = program(
        vop1("v_mov_b32", 1, ZERO),
        vop1("v_mov_b32", 2, ZERO),
        vop2("v_add_nc_u32", 4, ONE_INT, 0),                              # lane + 1
        *mimg_atomic("image_atomic_add", 4, 1, dmask=0x1, glc=0),
    )
    expected = run_interpreter(code, sgprs, memory)
    actual = rdna2_gpu.run(rdna2_hlsl.translate(code, sgprs), bytes(memory))
    assert words(actual, TEXTURE, 1) == words(expected, TEXTURE, 1) == [10 + 528]
