"""Hints that write no register: s_version (SOPK) and s_atc_probe / s_atc_probe_buffer (SMEM).

AMD "RDNA 2" ISA (November 2020): S_VERSION "Do nothing. Argument is ignored by hardware" (section 12.2,
SOPK opcode 1, p. 106; also section 4.1: "treated as S_NOP"); S_ATC_PROBE and S_ATC_PROBE_BUFFER "Probe or
prefetch an address into the SQC data cache" (section 12.6, SMEM opcodes 38-39, p. 130). Their SDATA field
is the probe mode (an immediate, `s_atc_probe 7, s[4:5], 0x64` in llvm-mc 20), not a register to write.
"""
import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import OPCODES, program, sop1, vop1
from test_rdna2_exec_memory import smem
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

LITERAL = 255
gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")
HINTS = ("s_version", "s_atc_probe", "s_atc_probe_buffer")


def sopk(name, sdst, simm16):
    return 0b1011 << 28 | OPCODES["SOPK", name] << 23 | sdst << 16 | (simm16 & 0xFFFF)


def hints():
    return (sopk("s_version", 0, 0x2000),
            *smem("s_atc_probe", 7, 4, offset=0x64),                       # SDATA 7 = mode, not s7
            *smem("s_atc_probe_buffer", 3, 8))


def test_hints_are_executable():
    assert all(rdna2_exec.executable(family, name)
               for family, name in (("SOPK", "s_version"), ("SMEM", "s_atc_probe"), ("SMEM", "s_atc_probe_buffer")))


def test_hints_change_no_register():
    setup = (sop1("s_mov_b32", 7, LITERAL), 0x1234_5678, sop1("s_mov_b32", 3, LITERAL), 0x9ABC_DEF0,
             vop1("v_mov_b32", 1, LITERAL), 0x0BAD_F00D)
    plain, hinted = rdna2_exec.run(program(*setup)), rdna2_exec.run(program(*setup, *hints()))
    assert (hinted.s, hinted.v, hinted.scc, hinted.exec, hinted.vcc) == (plain.s, plain.v, plain.scc, plain.exec,
                                                                         plain.vcc)
    assert (hinted.s[7], hinted.s[3]) == (0x1234_5678, 0x9ABC_DEF0)


def test_hlsl_translates_the_hints_to_nothing():
    plain = rdna2_hlsl.translate(program(vop1("v_mov_b32", 1, LITERAL), 5))
    hinted = rdna2_hlsl.translate(program(*hints(), vop1("v_mov_b32", 1, LITERAL), 5))
    assert hinted == plain


@gpu
def test_hints_match_interpreter_on_gpu():
    data = with_lane_offset(sop1("s_mov_b32", 7, LITERAL), 0x1234_5678, *hints(),
                            vop1("v_mov_b32", 1, 7), *store_lane(1, 0))
    expected, actual = compare(data)
    assert actual == expected
