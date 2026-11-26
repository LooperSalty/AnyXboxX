"""VOP3 clamp on the unsigned 32-bit integer add/subtract, in the reference interpreter and in HLSL.

AMD "RDNA 2" ISA, section 6.5 "ALU Clamp Bit Usage" (printed p. 48): for integer operations clamp "clamps the result
to the largest and smallest representable value", so v_add_nc_u32 / v_sub_nc_u32 / v_subrev_nc_u32 saturate D to
[0, 0xFFFFFFFF] instead of wrapping, and the carry-out forms (v_add_co_u32, v_sub_co_u32, v_subrev_co_u32 and the
_co_ci_ forms) saturate D to 0xFFFFFFFF on a carry or 0 on a borrow, while SDST still gets the carry/borrow bit
that the pseudo-code of sections 12.7 and 12.12 computes from the unbounded sum (our reading, not measured).
LLVM 20.1.8 (test/CodeGen/AMDGPU/uaddsat.ll, usubsat.ll) selects uaddsat/usubsat to these ops with clamp. Clamp on
the other integer results (v_mad_u32_u24, v_mad_u64_u32, v_cvt_u32_f32...) is not implemented and is refused.
"""
import random
import struct

import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import LITERAL, V, const, program, sop1, sop2, vop1, vop2, vop3
from test_rdna2_exec_memory import INPUT, OUTPUT, global_
from test_rdna2_hlsl import compare as compare32
from test_rdna2_hlsl_wave64 import compare as compare64

gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")
MASK32 = 0xFFFF_FFFF
NULL = 125
SDST, CARRY_IN = 40, 12                 # carry out in s[40] (s[40:41] in wave64), carry in from s[12] (s[12:13])
CARRY_IN_MASK = (0xAAAA_AAAA, 0x5555_5555)
NO_CARRY = ("v_add_nc_u32", "v_sub_nc_u32", "v_subrev_nc_u32")
CARRY = ("v_add_co_u32", "v_sub_co_u32", "v_subrev_co_u32", "v_add_co_ci_u32", "v_sub_co_ci_u32",
         "v_subrev_co_ci_u32")
# Edge pairs, each on an even lane (carry in 0) and the next odd lane (carry in 1); the other lanes are random.
EDGES = [(0, 0), (MASK32, 1), (MASK32, 0), (1, MASK32), (0x8000_0000, 0x8000_0000), (0x7FFF_FFFF, 0x8000_0000),
         (MASK32, MASK32), (5, 7), (7, 5), (9, 9), (0xFFFF_FFFE, 1), (0, 1), (1, 0)]


def rows(lanes):
    pairs = [pair for pair in EDGES for _ in range(2)]
    draw = random.Random(232 + lanes)
    return pairs + [(draw.getrandbits(32), draw.getrandbits(32)) for _ in range(lanes - len(pairs))]


def model(name, a, b, k):
    """(D, carry or borrow) of the op with clamp, from the exact unbounded result."""
    k = k if "_ci_" in name else 0
    if "_add" in name:
        total = a + b + k
        return (MASK32, 1) if total > MASK32 else (total, 0)
    total = (a - b if "_subrev" not in name else b - a) - k
    return (0, 1) if total < 0 else (total, 0)


def carry_in(lane):
    return CARRY_IN_MASK[lane >> 5] >> (lane & 31) & 1


def clamped(name, vdst=10):
    if name in NO_CARRY:
        return vop3(name, vdst, V + 1, V + 2, clamp=1)
    src2 = CARRY_IN if "_ci_" in name else 128
    return vop3(name, vdst, V + 1, V + 2, src2, sdst=SDST, clamp=1)


def build(lanes):
    """Program and input memory: every op of the two lists with clamp; D then the SDST words are stored."""
    slot_bytes = lanes * 4
    per_chunk = 2048 // slot_bytes            # the global_store offset is a signed 12-bit field
    words = [vop2("v_lshlrev_b32", 31, const(2), 0),
             *global_("global_load_dword", addr=31, vdst=1, saddr=0),
             *global_("global_load_dword", addr=31, vdst=2, saddr=0, offset=slot_bytes),
             sop1("s_mov_b32", CARRY_IN, LITERAL), CARRY_IN_MASK[0],
             sop1("s_mov_b32", CARRY_IN + 1, LITERAL), CARRY_IN_MASK[1]]
    stored = []

    def store(name, kind, register):
        index = len(stored)
        if index and index % per_chunk == 0:
            words.extend([sop2("s_add_u32", 2, 2, LITERAL), 2048])
        words.extend(global_("global_store_dword", addr=31, data=register, saddr=2,
                             offset=(index % per_chunk) * slot_bytes))
        stored.append((name, kind, register))

    for name in NO_CARRY + CARRY:
        words.extend(clamped(name))
        store(name, "d", 10)
        if name in CARRY:
            for half in range(lanes // 32):
                words.append(vop1("v_mov_b32", 11 + half, SDST + half))
                store(name, f"mask{half}", 11 + half)
    pairs = rows(lanes)
    memory = bytearray(INPUT + 2 * slot_bytes)
    struct.pack_into(f"<{lanes}I", memory, INPUT, *(a for a, _ in pairs))
    struct.pack_into(f"<{lanes}I", memory, INPUT + slot_bytes, *(b for _, b in pairs))
    return program(*words), bytes(memory), stored


def expected_outputs(lanes, stored):
    pairs = rows(lanes)
    out = []
    for name, kind, _ in stored:
        results = [model(name, a, b, carry_in(lane)) for lane, (a, b) in enumerate(pairs)]
        if kind == "d":
            out.append([d for d, _ in results])
        else:
            half = int(kind[-1])
            mask = sum(carry << lane for lane, (_, carry) in enumerate(results))
            out.append([mask >> (32 * half) & MASK32] * lanes)
    return out


def outputs(memory, lanes, count):
    return [list(struct.unpack_from(f"<{lanes}I", memory, OUTPUT + index * lanes * 4)) for index in range(count)]


def run_interpreter(data, memory, lanes):
    wave = rdna2_exec.Wave(lanes=lanes)
    image = bytearray(0x50000)
    image[:len(memory)] = memory
    wave.memory.write(0, bytes(image))
    wave.s[0], wave.s[2] = INPUT, OUTPUT
    rdna2_exec.run(data, wave)
    return wave.memory.read(0, 0x50000)


@pytest.mark.parametrize("lanes", [32, 64])
def test_interpreter_saturates_d_and_keeps_the_real_carry(lanes):
    data, memory, stored = build(lanes)
    actual = outputs(run_interpreter(data, memory, lanes), lanes, len(stored))
    for (name, kind, _), got, want in zip(stored, actual, expected_outputs(lanes, stored)):
        assert got == want, (name, kind)


@pytest.mark.parametrize("name, a, b, k, d, carry", [
    ("v_add_nc_u32", MASK32, 1, 0, MASK32, 0), ("v_add_nc_u32", 0x8000_0000, 0x7FFF_FFFF, 0, MASK32, 0),
    ("v_sub_nc_u32", 0, 1, 0, 0, 0), ("v_subrev_nc_u32", 1, 0, 0, 0, 0), ("v_sub_nc_u32", 7, 5, 0, 2, 0),
    ("v_add_co_u32", MASK32, 1, 0, MASK32, 1), ("v_sub_co_u32", 0, 1, 0, 0, 1),
    ("v_subrev_co_u32", 1, 0, 0, 0, 1), ("v_add_co_ci_u32", MASK32, 0, 1, MASK32, 1),
    ("v_add_co_ci_u32", 0xFFFF_FFFE, 0, 1, MASK32, 0), ("v_sub_co_ci_u32", 9, 9, 1, 0, 1),
    ("v_subrev_co_ci_u32", 9, 9, 1, 0, 1), ("v_sub_co_ci_u32", 10, 9, 1, 0, 0)])
def test_single_lane_values(name, a, b, k, d, carry):
    wave = rdna2_exec.run(program(sop1("s_mov_b32", 1, LITERAL), a, sop1("s_mov_b32", 2, LITERAL), b,
                                  sop1("s_mov_b32", CARRY_IN, const(k)),
                                  vop1("v_mov_b32", 1, 1), vop1("v_mov_b32", 2, 2), *clamped(name)))
    assert wave.v[10][0] == d
    if name in CARRY:
        assert wave.s[SDST] & 1 == carry


def test_without_clamp_the_ops_still_wrap():
    wave = rdna2_exec.run(program(sop1("s_mov_b32", 1, LITERAL), MASK32, vop1("v_mov_b32", 1, 1),
                                  *vop3("v_add_nc_u32", 10, V + 1, const(2)),
                                  *vop3("v_sub_co_u32", 11, const(1), V + 1, sdst=SDST)))
    assert wave.v[10][0] == 1 and wave.v[11][0] == 2 and wave.s[SDST] == MASK32


@pytest.mark.parametrize("wave_size", [32, 64])
def test_hlsl_saturates_from_the_carry_bit(wave_size):
    hlsl = rdna2_hlsl.translate(program(*clamped("v_add_nc_u32"), *clamped("v_sub_co_ci_u32")),
                                wave_size=wave_size)
    assert "v[10] = (v[1] + v[2] < v[1] ? 0xFFFFFFFFu : v[1] + v[2]);" in hlsl
    assert "v[10] = (o ? 0x0u : r);" in hlsl
    assert "clamp(" not in hlsl.split("[numthreads", 1)[1]


def refused_words(name):
    if name == "v_mad_u64_u32":
        return vop3(name, 4, V + 1, V + 2, V + 6, sdst=NULL, clamp=1)
    if name == "v_cvt_u32_f32":
        return vop3(name, 4, V + 1, clamp=1)
    return vop3(name, 4, V + 1, V + 2, V + 3, clamp=1)


REFUSED = ["v_mad_u32_u24", "v_mad_i32_i24", "v_mul_u32_u24", "v_cvt_u32_f32", "v_mad_u64_u32", "v_cndmask_b32"]


@pytest.mark.parametrize("name", REFUSED)
def test_other_integer_clamp_is_refused_by_name(name):
    data = program(*refused_words(name))
    with pytest.raises(rdna2_exec.Untranslated, match=f"{name} with clamp on an integer result"):
        rdna2_exec.run(data)
    with pytest.raises(rdna2_exec.Untranslated, match=f"{name} with clamp on an integer result"):
        rdna2_exec.run(data, rdna2_exec.Wave(lanes=64))
    for wave_size in (32, 64):
        with pytest.raises(rdna2_hlsl.Untranslatable, match=f"{name} with clamp on an integer result"):
            rdna2_hlsl.translate(data, wave_size=wave_size)


@gpu
@pytest.mark.parametrize("lanes", [32, 64])
def test_gpu_matches_the_interpreter_and_the_isa_rule(lanes):
    data, memory, stored = build(lanes)
    expected, actual = (compare32(data, memory=memory) if lanes == 32 else compare64(data, memory=memory))
    assert outputs(expected, lanes, len(stored)) == expected_outputs(lanes, stored)
    assert actual == expected
