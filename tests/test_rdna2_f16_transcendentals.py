"""16-bit float transcendentals (VOP1 and VOP3): interpreter, HLSL translation and GPU parity.

AMD "RDNA 2" Instruction Set Architecture (November 2020), section 12.8, VOP1 opcodes 84-88 and 96-97
(printed pages 147-149): v_rcp_f16, v_sqrt_f16, v_rsq_f16, v_log_f16, v_exp_f16, v_sin_f16, v_cos_f16.
All NaN results are canonicalised to 0x7E00 (matching emu/rdna2_exec_f16.py and emu/rdna2_hlsl_f16.py).
v_sin_f16 and v_cos_f16 operate in turns (sin/cos(S0.f16 * 2 * pi)) with exact turn reduction before
multiplying by 2 * pi so large integers (e.g. +-65504) and quarter-turns match the ISA examples.
"""
import struct

import pytest

import test_rdna2_hlsl_wave64 as wave64
from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl, rdna2_hlsl_graphics
from test_rdna2_exec import EXEC_LO, LITERAL, ONE, TWO, V, const, program, sop1, sop2, sopc, sopp, vop1, vop2, vop3
from test_rdna2_exec_memory import INPUT, global_
from test_rdna2_graphics import PIXEL_SHADER, VERTEX_SHADER, WIDTH
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc or anyxboxx_gpu_run.exe missing")
OLD = 0xCAFE_0000
NAN = 0x7E00
NAMES = ("v_rcp_f16", "v_sqrt_f16", "v_rsq_f16", "v_log_f16", "v_exp_f16", "v_sin_f16", "v_cos_f16")

# Hand-verified expected outputs from the ISA functional examples (pp. 147-149) and IEEE 754 binary16.
ISA_CASES = {
    "v_rcp_f16": [
        (0xFC00, 0x8000),  # rcp(-INF) = -0.0
        (0xC000, 0xB800),  # rcp(-2.0) = -0.5
        (0x8000, 0xFC00),  # rcp(-0.0) = -INF
        (0x0000, 0x7C00),  # rcp(+0.0) = +INF
        (0x7C00, 0x0000),  # rcp(+INF) = +0.0
        (0x4400, 0x3400),  # rcp(4.0) = 0.25
        (0x0001, 0x7C00),  # rcp(2^-24) overflows -> +INF
        (0x7BFF, 0x0100),  # rcp(65504.0) -> subnormal 2^-16
        (0x7E01, NAN),     # rcp(NaN) = NaN
    ],
    "v_sqrt_f16": [
        (0xFC00, NAN),     # sqrt(-INF) = NaN
        (0xBC00, NAN),     # sqrt(-1.0) = NaN
        (0x8001, NAN),     # sqrt(-2^-24) = NaN
        (0x8000, 0x8000),  # sqrt(-0.0) = -0.0
        (0x0000, 0x0000),  # sqrt(+0.0) = +0.0
        (0x4400, 0x4000),  # sqrt(4.0) = 2.0
        (0x3400, 0x3800),  # sqrt(0.25) = 0.5
        (0x0001, 0x0C00),  # sqrt(2^-24) = 2^-12
        (0x7C00, 0x7C00),  # sqrt(+INF) = +INF
        (0xFE01, NAN),     # sqrt(NaN) = NaN
    ],
    "v_rsq_f16": [
        (0xFC00, NAN),     # rsq(-INF) = NaN
        (0xBC00, NAN),     # rsq(-1.0) = NaN
        (0x8000, 0xFC00),  # rsq(-0.0) = -INF
        (0x0000, 0x7C00),  # rsq(+0.0) = +INF
        (0x4400, 0x3800),  # rsq(4.0) = 0.5
        (0x3400, 0x4000),  # rsq(0.25) = 2.0
        (0x0001, 0x6C00),  # rsq(2^-24) = 2^12 = 4096
        (0x7C00, 0x0000),  # rsq(+INF) = +0.0
        (0x7D00, NAN),     # rsq(NaN) = NaN
    ],
    "v_log_f16": [
        (0xFC00, NAN),     # log2(-INF) = NaN
        (0xBC00, NAN),     # log2(-1.0) = NaN
        (0x8000, 0xFC00),  # log2(-0.0) = -INF
        (0x0000, 0xFC00),  # log2(+0.0) = -INF
        (0x3C00, 0x0000),  # log2(1.0) = +0.0
        (0x4400, 0x4000),  # log2(4.0) = 2.0
        (0x3400, 0xC000),  # log2(0.25) = -2.0
        (0x0001, 0xCE00),  # log2(2^-24) = -24.0
        (0x7C00, 0x7C00),  # log2(+INF) = +INF
        (0x7E00, NAN),     # log2(NaN) = NaN
    ],
    "v_exp_f16": [
        (0xFC00, 0x0000),  # exp2(-INF) = +0.0
        (0x8000, 0x3C00),  # exp2(-0.0) = 1.0
        (0x0000, 0x3C00),  # exp2(+0.0) = 1.0
        (0x3C00, 0x4000),  # exp2(1.0) = 2.0
        (0xBC00, 0x3800),  # exp2(-1.0) = 0.5
        (0x11C5, 0x3C01),  # exp2(1477 * 2^-21) > 1 + 2^-11 (double-rounding boundary)
        (0xCE00, 0x0001),  # exp2(-24.0) = 2^-24 (smallest subnormal)
        (0xCE40, 0x0000),  # exp2(-25.0) underflows -> +0.0
        (0x4B80, 0x7800),  # exp2(15.0) = 32768.0
        (0x4C00, 0x7C00),  # exp2(16.0) overflows -> +INF
        (0x7C00, 0x7C00),  # exp2(+INF) = +INF
        (0x7E01, NAN),     # exp2(NaN) = NaN
    ],
    "v_sin_f16": [
        (0xFC00, NAN),     # sin(-INF) = NaN
        (0xFBFF, 0x0000),  # sin(-65504.0 * 2pi) = +0.0
        (0x8000, 0x8000),  # sin(-0.0) = -0.0
        (0x0000, 0x0000),  # sin(+0.0) = +0.0
        (0x3400, 0x3C00),  # sin(0.25 * 2pi) = 1.0
        (0xB400, 0xBC00),  # sin(-0.25 * 2pi) = -1.0
        (0x3800, 0x0000),  # sin(0.5 * 2pi) = +0.0
        (0x3A00, 0xBC00),  # sin(0.75 * 2pi) = -1.0
        (0x3000, 0x39A8),  # sin(0.125 * 2pi) = sqrt(2)/2 = 0.70703125
        (0x0001, 0x0006),  # sin(2^-24 * 2pi) = 6 * 2^-24 (positive subnormal)
        (0x8001, 0x8006),  # sin(-2^-24 * 2pi) = -6 * 2^-24 (negative subnormal)
        (0x7BFF, 0x0000),  # sin(+65504.0 * 2pi) = +0.0
        (0x7C00, NAN),     # sin(+INF) = NaN
        (0x7E00, NAN),     # sin(NaN) = NaN
    ],
    "v_cos_f16": [
        (0xFC00, NAN),     # cos(-INF) = NaN
        (0xFBFF, 0x3C00),  # cos(-65504.0 * 2pi) = 1.0
        (0x8000, 0x3C00),  # cos(-0.0) = 1.0
        (0x0000, 0x3C00),  # cos(+0.0) = 1.0
        (0x3400, 0x0000),  # cos(0.25 * 2pi) = +0.0
        (0xB400, 0x0000),  # cos(-0.25 * 2pi) = +0.0
        (0x3800, 0xBC00),  # cos(0.5 * 2pi) = -1.0
        (0x3A00, 0x0000),  # cos(0.75 * 2pi) = +0.0
        (0x3000, 0x39A8),  # cos(0.125 * 2pi) = sqrt(2)/2
        (0x8001, 0x3C00),  # cos(-2^-24 * 2pi) = 1.0
        (0x7BFF, 0x3C00),  # cos(+65504.0 * 2pi) = 1.0
        (0x7C00, NAN),     # cos(+INF) = NaN
        (0x7E00, NAN),     # cos(NaN) = NaN
    ],
}

# 32 lanes covering special values, subnormals, exact quarter-turns, general finite values and the 0x11C5 boundary.
GPU_INPUTS = [
    0x0000, 0x8000, 0x3C00, 0xBC00, 0x4000, 0xC000, 0x4400, 0xC400,
    0x3800, 0xB800, 0x3400, 0xB400, 0x3A00, 0xBA00, 0x3000, 0xB000,
    0x3E00, 0xBE00, 0x11C5, 0x4800, 0xCE00, 0xCE40, 0x4B80, 0x4C00,
    0x0001, 0x8001, 0x03FF, 0x7BFF, 0xFBFF, 0x7C00, 0xFC00, 0x7E01,
]


def run_wave(values, *words, lanes=32, exec_mask=None):
    wave = rdna2_exec.Wave(lanes=lanes)
    if exec_mask is not None:
        wave.write_mask(EXEC_LO, exec_mask)
    wave.v[1] = [OLD | values[lane % len(values)] for lane in range(lanes)]
    wave.v[2] = [values[lane % len(values)] << 16 | 0x55AA for lane in range(lanes)]
    wave.v[3] = [OLD | 0xBEEF] * lanes
    rdna2_exec.run(program(*words), wave)
    return wave


@pytest.mark.parametrize("name", NAMES)
def test_vop1_matches_isa_examples_and_preserves_high_half(name):
    pairs = ISA_CASES[name]
    inputs, expected = [x for x, _ in pairs], [y for _, y in pairs]
    wave = run_wave(inputs, vop1(name, 3, V + 1))
    assert wave.v[3] == [OLD | expected[lane % len(expected)] for lane in range(32)]
    assert rdna2_exec.executable("VOP1", name) and rdna2_exec.executable("VOP3", name)


@pytest.mark.parametrize("name", NAMES)
def test_vop3_op_sel_abs_neg_and_clamp(name):
    pairs = ISA_CASES[name]
    inputs, expected = [x for x, _ in pairs], [y for _, y in pairs]
    # op_sel=0b1001: read high half of v2, write high half of v3 while keeping low half 0xBEEF
    wave = run_wave(inputs, *vop3(name, 3, V + 2, op_sel=0b1001))
    assert wave.v[3] == [expected[lane % len(expected)] << 16 | 0xBEEF for lane in range(32)]
    # clamp=1 clamps result to [0, 1] (negatives and NaN become +0)
    clamped = run_wave([0x4400, 0xC400, 0x3400, 0x7E00], *vop3(name, 3, V + 1, clamp=1))
    for lane in range(4):
        result = clamped.v[3][lane] & 0xFFFF
        assert result <= 0x3C00 and (result & 0x8000) == 0


def test_vop3_modifiers_and_inline_constants():
    # abs(-4.0) = 4.0 -> sqrt = 2.0 (0x4000); neg(2.0) = -2.0 -> rcp = -0.5 (0xB800); exp2(1.0) = 2.0
    wave = run_wave([0xC400],
                    *vop3("v_sqrt_f16", 4, V + 1, abs_=0b001),
                    *vop3("v_rcp_f16", 5, TWO, neg=0b001),
                    vop1("v_exp_f16", 6, ONE),
                    vop1("v_log_f16", 7, LITERAL), 0xDEAD_4400)
    assert wave.v[4][0] & 0xFFFF == 0x4000
    assert wave.v[5][0] & 0xFFFF == 0xB800
    assert wave.v[6][0] & 0xFFFF == 0x4000
    assert wave.v[7][0] & 0xFFFF == 0x4000


def test_exec_mask_preserves_inactive_lanes():
    mask = 0x0F0F_55AA
    wave = run_wave([0x4400], vop1("v_sqrt_f16", 3, V + 1), exec_mask=mask)
    expected = [OLD | (0x4000 if mask >> lane & 1 else 0xBEEF) for lane in range(32)]
    assert wave.v[3] == expected


@pytest.mark.parametrize("name", NAMES)
def test_refused_forms_fail_in_interpreter_and_hlsl(name):
    for words, pattern in [
        ((vop1(name, 3, 0xF9), 0), "SDWA/DPP"),
        (vop3(name, 3, V + 1, omod=1), "omod"),
    ]:
        with pytest.raises(rdna2_exec.Untranslated, match=pattern):
            rdna2_exec.run(program(*words))
        with pytest.raises(rdna2_hlsl.Untranslatable, match=pattern):
            rdna2_hlsl.translate(program(*words))


def test_hlsl_translation_compute_wave64_and_graphics():
    assert set(NAMES) <= rdna2_hlsl.coverage_names()
    body = [vop1(name, 3, V + 1) for name in NAMES]
    body += [word for name in NAMES for word in vop3(name, 4, V + 2, op_sel=0b1001, abs_=1, clamp=1)]
    hlsl32 = rdna2_hlsl.translate(program(*body))
    hlsl64 = rdna2_hlsl.translate(program(*body), wave_size=64)
    for helper in ("F16Rcp(", "F16Sqrt(", "F16Rsq(", "F16Log(", "F16Exp(", "F16Trig("):
        assert helper in hlsl32 and helper in hlsl64
    assert "uint F16Trig(uint h, bool is_cos)" in rdna2_hlsl_graphics.translate_vertex(VERTEX_SHADER)
    assert "uint F16Trig(uint h, bool is_cos)" in rdna2_hlsl_graphics.translate_pixel(PIXEL_SHADER, WIDTH)


def _gpu_memory(lanes=32):
    memory = bytearray(INPUT + lanes * 4)
    for lane in range(lanes):
        val = GPU_INPUTS[lane % len(GPU_INPUTS)]
        struct.pack_into("<I", memory, INPUT + lane * 4, ((0xA500 | lane) << 16) | val)
    return bytes(memory)


@gpu
def test_f16_transcendentals_match_interpreter_on_gpu():
    words = [*global_("global_load_dword", addr=31, vdst=1, saddr=0)]
    for slot, name in enumerate(NAMES):
        words += [vop1("v_mov_b32", 10, LITERAL), OLD | 0x1234, vop1(name, 10, V + 1), *store_lane(10, slot)]
    for offset, name in enumerate(NAMES):
        words += [vop1("v_mov_b32", 10, LITERAL), 0x1234_5678,
                  *vop3(name, 10, V + 1, op_sel=0b1000, abs_=1, clamp=1), *store_lane(10, len(NAMES) + offset)]
    expected, actual = compare(with_lane_offset(*words), memory=_gpu_memory(32))
    assert actual == expected


@gpu
def test_f16_transcendentals_match_interpreter_in_wave64_on_gpu():
    words = [
        *global_("global_load_dword", addr=31, vdst=1, saddr=0),
        sop1("s_mov_b32", EXEC_LO, LITERAL), 0xF0F0_FF0F,
        vop1("v_rcp_f16", 2, V + 1),
        vop1("v_sin_f16", 3, V + 1),
        vop1("v_cos_f16", 4, V + 1),
        sop1("s_mov_b32", EXEC_LO, 193),
        *wave64.store_lane(2, 0), *wave64.store_lane(3, 1), *wave64.store_lane(4, 2),
    ]
    expected, actual = wave64.compare(wave64.with_lane_offset(*words), memory=_gpu_memory(64))
    assert actual == expected


def _exhaustive_program(name):
    """Wave32 loop sweeping all 65,536 binary16 bit patterns (0x0000..0xFFFF) into u0[h * 4]."""
    loop_body = [
        vop2("v_add_nc_u32", 1, 4, 0),
        vop2("v_lshlrev_b32", 31, const(2), 1),
        vop1("v_mov_b32", 2, const(0)),
        vop1(name, 2, V + 1),
        *global_("global_store_dword", addr=31, data=2, saddr=2),
        sop2("s_add_u32", 4, 4, const(32)),
        sopc("s_cmp_lt_u32", 4, 5),
    ]
    return program(
        sop1("s_mov_b32", 4, const(0)),
        sop1("s_mov_b32", 5, LITERAL), 0x10000,
        *loop_body,
        sopp("s_cbranch_scc1", -(len(loop_body) + 1)),
    )


@pytest.mark.parametrize("name", NAMES)
def test_exhaustive_program_translates_cleanly(name):
    hlsl = rdna2_hlsl.translate(_exhaustive_program(name), {2: 0})
    assert "while" in hlsl and "switch (pc)" not in hlsl


@gpu
@pytest.mark.parametrize("name", NAMES)
def test_f16_transcendentals_exhaustive_parity_on_gpu(name):
    """Exhaustive interpreter vs HLSL (GPU/WARP) parity over all 65,536 binary16 inputs."""
    data = _exhaustive_program(name)
    size = 65536 * 4
    wave = rdna2_exec.Wave()
    wave.memory.write(0, bytes(size))
    wave.s[2] = 0
    rdna2_exec.run(data, wave)
    expected = struct.unpack("<65536I", wave.memory.read(0, size))
    actual = struct.unpack("<65536I", rdna2_gpu.run(rdna2_hlsl.translate(data, {2: 0}), bytes(size)))
    mismatches = [
        (hex(h), hex(expected[h]), hex(actual[h]))
        for h in range(65536)
        if actual[h] != expected[h]
    ]
    assert not mismatches, f"{name}: {len(mismatches)} mismatches (first 10: {mismatches[:10]})"
