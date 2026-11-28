"""Regression tests for out-of-bounds 64-bit / 128-bit VGPR groups in the Python RDNA 2 model.

Matches the Rust reference interpreter (#437): multi-register VGPR groups that extend past v255
must raise ``Untranslated`` before mutating any lane instead of raising ``IndexError`` after
writing ``v[255]``.
"""
import struct

import pytest

from emu.rdna2_exec import Untranslated, Wave, run
from emu.rdna2_ops import MNEMONICS

OPCODES = {(family, name): opcode for (family, opcode), name in MNEMONICS.items()}
V0 = 256
ENDPGM = (0b1011_1111_1000 << 20) | (0x01 << 16)


def _vop3(name: str, vdst: int, src0: int, src1: int, src2: int = 0, sdst: int = 0) -> bytes:
    opcode = OPCODES["VOP3", name]
    w0 = (0b1101_01 << 26) | ((opcode & 0x3FF) << 16) | ((sdst & 0x7F) << 8) | (vdst & 0xFF)
    w1 = ((src2 & 0x1FF) << 18) | ((src1 & 0x1FF) << 9) | (src0 & 0x1FF)
    return struct.pack("<3I", w0, w1, ENDPGM)


@pytest.mark.parametrize(
    ("name", "vdst", "src0", "src1", "src2", "sdst"),
    [
        # 64-bit f64 destination at v255 (rdna2_exec_wide.py)
        ("v_add_f64", 255, V0, V0 + 2, 0, 0),
        # 64-bit shift destination at v255 (rdna2_exec_wide.py)
        ("v_lshlrev_b64", 255, 129, V0, 0, 0),
        # 64-bit div destination at v255 (rdna2_exec_div.py)
        ("v_div_fixup_f64", 255, V0, V0 + 2, V0 + 4, 0),
        # 64-bit mad destination at v255 (rdna2_exec_vop3.py)
        ("v_mad_u64_u32", 255, V0, V0 + 1, V0 + 2, 106),
        # 64-bit packed SAD destination at v255 (rdna2_exec_sad.py)
        ("v_qsad_pk_u16_u8", 255, V0, V0 + 2, V0 + 4, 0),
        # 64-bit packed SAD S2 pair at v255 (rdna2_exec_sad.py)
        ("v_qsad_pk_u16_u8", 4, V0, V0 + 2, V0 + 255, 0),
        # 128-bit mqsad destination at v253..v255 (rdna2_exec_sad.py)
        ("v_mqsad_u32_u8", 253, V0, V0 + 2, V0 + 4, 0),
        # 128-bit mqsad S2 quad at v253..v255 (rdna2_exec_sad.py)
        ("v_mqsad_u32_u8", 4, V0, V0 + 2, V0 + 253, 0),
    ],
)
def test_vgpr_group_past_v255_raises_untranslated_without_mutation(
    name: str, vdst: int, src0: int, src1: int, src2: int, sdst: int
) -> None:
    wave = Wave(lanes=32)
    sentinel = 0xCAFE_BABE
    wave.v[255][0] = sentinel
    wave.v[253][0] = sentinel
    words = _vop3(name, vdst, src0, src1, src2, sdst)
    with pytest.raises(Untranslated):
        run(words, wave=wave)
    assert wave.v[255][0] == sentinel
    assert wave.v[253][0] == sentinel
