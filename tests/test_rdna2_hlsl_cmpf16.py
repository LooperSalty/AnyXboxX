"""Half-precision compares in the HLSL recompiler: text checks on the CPU and byte-for-byte GPU parity.

The interpreter's semantics (emu/rdna2_exec_cmpf16.py, tests/test_rdna2_exec_cmpf16.py) are the oracle: every
mask is computed by both and compared. Inputs cover ±0, NaNs of both kinds, denormals, ±inf and neighbours,
with junk high halves.
"""
import struct

import pytest

import test_rdna2_hlsl_wave64 as wave64
from emu import rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import V, program, sop1, vop1, vop2, vop3, vopc
from test_rdna2_exec_cmpf16 import CLASSES, HOLDS, PAIRS
from test_rdna2_exec_memory import INPUT, OUTPUT, global_
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

LITERAL, ONE, MINUS_ONE, VCC_LO, EXEC_LO = 255, 242, 193, 106, 126
SDWA, PARTIAL_EXEC, ALL = 0xF9, 0x0F0F_F0F3, 0xFFFF_FFFF
SLOTS_PER_BASE = 16
NAMES = [f"v_cmp_{operation}_f16" for operation in (*HOLDS, "class")]
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc or anyxboxx_gpu_run.exe missing")


def inputs(count):
    """(S0, S1) per lane: the hand-labelled pairs, then each class value against single class bits."""
    first = [a for a, _, _ in PAIRS] + [0x5A5A_0000 | value for value in CLASSES]
    second = [b for _, b, _ in PAIRS] + [0xA5A5_0000 | 1 << (index * 7 % 10) for index in range(len(CLASSES))]
    while len(first) < count:
        lane = len(first)
        first.append((lane * 2654435761) & 0xFFFF_FFFF)
        second.append(first[lane % len(PAIRS)] if lane % 3 == 0 else (lane * 40503 << 7) & 0xFFFF_FFFF)
    image = bytearray(INPUT + 8 * count)
    struct.pack_into(f"<{2 * count}I", image, INPUT, *first[:count], *second[:count])
    return bytes(image)


def loads(count):
    return [*global_("global_load_dword", addr=31, vdst=1, saddr=0),
            *global_("global_load_dword", addr=31, vdst=2, saddr=0, offset=4 * count)]


def store_masks(sgprs, store=store_lane):
    words = []
    for slot, sgpr in enumerate(sgprs):
        if slot and slot % SLOTS_PER_BASE == 0:
            words += [vop2("v_add_nc_u32", 31, LITERAL, 31), SLOTS_PER_BASE * 128]
        words += [vop1("v_mov_b32", 3, sgpr), *store(3, slot % SLOTS_PER_BASE)]
    return words


def compares(names, encodings=("VOPC", "VOP3")):
    """Each compare and its cmpx form under a partial EXEC; masks in s40 onwards."""
    words, sgpr = [], 40
    for name in names:
        for opcode in (name, name.replace("v_cmp_", "v_cmpx_")):
            for encoding in encodings:
                words += [sop1("s_mov_b32", EXEC_LO, LITERAL), PARTIAL_EXEC]
                writes_exec = opcode.startswith("v_cmpx")
                target = EXEC_LO if writes_exec else sgpr
                words += [vopc(opcode, V + 1, 2)] if encoding == "VOPC" else [*vop3(opcode, target, V + 1, V + 2)]
                if writes_exec or encoding == "VOPC":
                    words.append(sop1("s_mov_b32", sgpr, EXEC_LO if writes_exec else VCC_LO))
                sgpr += 1
    return words, sgpr


def parity_program(names):
    words, end = compares(names)
    return with_lane_offset(*loads(32), *words, sop1("s_mov_b32", EXEC_LO, LITERAL), ALL,
                            *store_masks(range(40, end)))


def test_predicates_widen_both_halves_and_class_reads_the_bits():
    hlsl = rdna2_hlsl.translate(program(vopc("v_cmp_nge_f16", V + 1, 2),
                                        *vop3("v_cmp_class_f16", 40, V + 1, V + 2)))
    assert "IsNanF32(F16ToF32((v[1] & 0xFFFFu)))" in hlsl and "F16ToF32((v[2] & 0xFFFFu))" in hlsl
    assert "ClassF16((v[1] & 0xFFFFu))" in hlsl and "uint ClassF16(uint h)" in hlsl


def test_constants_and_modifiers_follow_the_f16_rules():
    hlsl = rdna2_hlsl.translate(program(vopc("v_cmp_eq_f16", ONE, 1), vopc("v_cmp_eq_f16", LITERAL, 1), 0xDEAD_3C00,
                                        *vop3("v_cmp_lt_f16", 40, V + 1, V + 2, abs_=0b001, neg=0b010)))
    assert "F16ToF32(0x3C00u)" in hlsl                         # inline 1.0 as binary16
    assert "F16ToF32((0xDEAD3C00u & 0xFFFFu))" in hlsl         # a literal's low half
    assert "((v[1] & 0xFFFFu) & 0x7FFFu)" in hlsl and "((v[2] & 0xFFFFu) ^ 0x8000u)" in hlsl


@pytest.mark.parametrize("words, reason", [
    ((vopc("v_cmp_lt_f16", SDWA, 2), 1), "SDWA"),
    (vop3("v_cmp_lt_f16", 40, V + 1, V + 2, op_sel=0b0001), "op_sel"),
    (vop3("v_cmp_lt_f16", 40, V + 1, V + 2, clamp=1), "clamp"),
    (vop3("v_cmp_class_f16", 40, V + 1, V + 2, neg=0b010), "class mask"),
])
def test_unmeasured_forms_are_refused_by_name(words, reason):
    with pytest.raises(rdna2_hlsl.Untranslatable, match=reason):
        rdna2_hlsl.translate(program(*words))


def test_wave64_translation_uses_64_lane_masks():
    data = program(vopc("v_cmp_lt_f16", V + 1, 2), *vop3("v_cmpx_class_f16", EXEC_LO, V + 1, V + 2))
    hlsl = rdna2_hlsl.translate(data, wave_size=64)
    assert hlsl.count("Ballot64(lane,") >= 2 and "ClassF16(" in hlsl


@gpu
@pytest.mark.parametrize("names", [NAMES[:9], NAMES[9:]])
def test_f16_compares_match_interpreter_on_gpu(names):
    expected, actual = compare(parity_program(names), memory=inputs(32))
    assert actual == expected


@gpu
def test_constants_and_modifiers_match_interpreter_on_gpu():
    words = [vopc("v_cmp_eq_f16", ONE, 1), sop1("s_mov_b32", 40, VCC_LO),
             vopc("v_cmp_le_f16", LITERAL, 1), 0xDEAD_3C00, sop1("s_mov_b32", 41, VCC_LO),
             *vop3("v_cmp_lt_f16", 42, V + 1, V + 2, abs_=0b001, neg=0b010),
             *vop3("v_cmp_class_f16", 43, V + 1, 240),            # inline 0.5 = 0x3800 as the mask
             *vop3("v_cmp_nlg_f16", 44, MINUS_ONE, V + 2)]
    data = with_lane_offset(*loads(32), *words, *store_masks(range(40, 45)))
    expected, actual = compare(data, memory=inputs(32))
    assert actual == expected


@gpu
def test_wave64_f16_compares_match_interpreter_on_gpu():
    exec_lo, exec_hi = 0xF0F0_FFF3, 0x0FF0_0FF1
    set_exec = [sop1("s_mov_b32", EXEC_LO, LITERAL), exec_lo, sop1("s_mov_b32", EXEC_LO + 1, LITERAL), exec_hi]
    words = [*set_exec,
             vopc("v_cmp_lt_f16", V + 1, 2), sop1("s_mov_b32", 40, VCC_LO), sop1("s_mov_b32", 41, VCC_LO + 1),
             *vop3("v_cmp_neq_f16", 42, V + 1, V + 2),
             *vop3("v_cmp_class_f16", 44, V + 1, V + 2),
             *vop3("v_cmpx_u_f16", EXEC_LO, V + 1, V + 2), sop1("s_mov_b32", 46, EXEC_LO),
             sop1("s_mov_b32", 47, EXEC_LO + 1),
             sop1("s_mov_b32", EXEC_LO, MINUS_ONE), sop1("s_mov_b32", EXEC_LO + 1, MINUS_ONE)]
    stores = [word for slot, sgpr in enumerate(range(40, 48))
              for word in (vop1("v_mov_b32", 3, sgpr), *wave64.store_lane(3, slot))]
    data = wave64.with_lane_offset(*loads(64), *words, *stores)
    expected, actual = wave64.compare(data, memory=inputs(64))
    assert actual == expected
    assert struct.unpack_from("<I", expected, OUTPUT)[0] != 0
