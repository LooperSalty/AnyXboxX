"""16-bit typed buffer accesses of the RDNA 2 interpreter (buffer_*_format_d16_*, tbuffer_*_format_d16_*).

Expected values are computed by hand from the rules of emu/rdna2_buffer_d16.py (sources there).
float16 reminders: 1.0 = 0x3C00, -2.0 = 0xC000, 0.2 = 0x3266, 65504 (largest) = 0x7BFF,
+inf = 0x7C00, 2^-24 (smallest subnormal) = 0x0001, 2^-14 (smallest normal) = 0x0400.
"""
import random
import struct
from fractions import Fraction

import pytest

from emu import rdna2_buffer_d16 as d16, rdna2_buffer_format as formats, rdna2_exec
from test_rdna2_exec import program, sop1, vop1
from test_rdna2_exec_format import XYZW, typed, word3
from test_rdna2_exec_memory import BUFFER

RSRC, LITERAL, EXEC_LO = 20, 255, 126
ONE_HALF, INFINITY = 0x3C00, 0x7C00
FILL = 0xDEADBEEF                       # VGPR contents before a load: the halves not written stay


def run(words, memory=b"", records=0, stride=0, desc3=0, setup=()):
    """Program `setup` + `words` with the V# in s20-s23; v8-v11 start at FILL."""
    wave = rdna2_exec.Wave()
    wave.memory.write(BUFFER, memory)
    wave.s[RSRC:RSRC + 4] = [BUFFER, stride << 16, records, desc3]
    for register in range(8, 12):
        wave.v[register] = [FILL] * 32
    return rdna2_exec.run(program(*setup, *words), wave)


def lane0(wave, first=8, count=2):
    return [wave.v[first + i][0] for i in range(count)]


def floats(*bits):
    return struct.pack(f"<{len(bits)}I", *bits)


# --- float32 → float16 (assumed, see emu/rdna2_buffer_d16.py) ----------------------------
# (bits, round to nearest even: SCALED formats, toward zero: FLOAT formats)

@pytest.mark.parametrize("bits, nearest, toward_zero", [
    (0x3F800000, 0x3C00, 0x3C00),       # 1.0
    (0xC2280000, 0xD140, 0xD140),       # -42.0
    (0x477FE000, 0x7BFF, 0x7BFF),       # 65504, the largest float16
    (0x477FEFFF, 0x7BFF, 0x7BFF),       # just under 65520
    (0x477FF000, INFINITY, 0x7BFF),     # 65520: halfway to 65536, to even = infinity; toward zero 65504
    (0xC77FF000, 0xFC00, 0xFBFF),       # -65520
    (0x501502F9, INFINITY, 0x7BFF),     # 1e10
    (0xFF800000, 0xFC00, 0xFC00),       # -infinity
    (0x80000000, 0x8000, 0x8000),       # -0.0
    (0x00000001, 0x0000, 0x0000),       # float32 subnormal: far below 2^-25
    (0x3F801000, 0x3C00, 0x3C00),       # 1 + 2^-11: halfway, to even (mantissa 0)
    (0x3F803000, 0x3C02, 0x3C01),       # 1 + 3 × 2^-11: halfway, to even (mantissa 2); truncated 1
    (0x38800000, 0x0400, 0x0400),       # 2^-14, smallest normal
    (0x387FC000, 0x03FF, 0x03FF),       # largest subnormal
    (0x33800000, 0x0001, 0x0001),       # 2^-24
    (0x33000000, 0x0000, 0x0000),       # 2^-25: halfway between 0 and 2^-24, to even
    (0x33000001, 0x0001, 0x0000),       # just above 2^-25
    (0x33C00000, 0x0002, 0x0001),       # 1.5 × 2^-24: halfway, to even; truncated 2^-24
    (0x7FC00000, 0x7E00, 0x7E00),       # quiet NaN: sign and top 10 payload bits
    (0x7F802000, 0x7C01, 0x7C01),       # signalling NaN: payload 1 << 13 → 1, no quiet bit added
    (0xFFC00001, 0xFE00, 0xFE00),       # low payload bits dropped
    (0x7F800001, 0x7C01, 0x7C01),       # top 10 payload bits zero: payload 1, still a NaN
])
def test_float32_narrowing(bits, nearest, toward_zero):
    assert d16.half_from_float(bits) == nearest
    assert d16.half_from_float(bits, toward_zero=True) == toward_zero
    # Through the instruction: 32_FLOAT (toward zero), d16_x writes bits 15-0 and keeps FILL's high half.
    wave = run(typed("buffer_load_format_d16_x"), floats(bits), records=4, desc3=word3(22, 4))
    assert wave.v[8][0] == 0xDEAD0000 | toward_zero


def test_narrowing_matches_python_binary16_rounding():
    # Independent oracle: struct's "e" format rounds a double to binary16, ties to even, and raises
    # OverflowError beyond the format (infinity under round to nearest even).
    generator = random.Random(16)
    samples = [generator.getrandbits(32) for _ in range(20000)] + list(range(0x33000000, 0x38800000, 0x1001))
    for bits in samples:
        if (bits >> 23) & 0xFF == 0xFF:
            continue
        value = struct.unpack("<f", struct.pack("<I", bits))[0]
        try:
            expected = struct.unpack("<H", struct.pack("<e", value))[0]
        except OverflowError:
            expected = INFINITY | (0x8000 if value < 0 else 0)
        assert d16.half_from_float(bits) == expected, hex(bits)


def test_narrowing_toward_zero_keeps_the_largest_half_not_above():
    # Independent oracle: among struct's binary16 rounding and its neighbours, the largest magnitude not
    # above |value|; past 65504, 65504.
    generator = random.Random(17)
    samples = [generator.getrandbits(32) for _ in range(20000)] + list(range(0x33000000, 0x38800000, 0x1001))
    for bits in samples:
        if (bits >> 23) & 0xFF == 0xFF:
            continue
        value = abs(struct.unpack("<f", struct.pack("<I", bits))[0])
        sign = bits >> 16 & 0x8000
        if value >= 65504:
            expected = 0x7BFF
        else:
            guess = struct.unpack("<H", struct.pack("<e", value))[0]
            expected = max(h for h in (guess - 1, guess, guess + 1) if 0 <= h <= 0x7BFF and half_fraction(h) <= value)
        assert d16.half_from_float(bits, toward_zero=True) == sign | expected, hex(bits)


# --- Normalized, scaled and integer formats ----------------------------------------------

# d16 UNORM/SNORM loads round the exact quotient once: assumed, to be measured in akimfomin/PortBridge#85.
# The four 16-bit values where narrowing the float32 would differ:
ONE_ROUNDING_SPLITS = [
    (7, 0xFFAF, 0x3BFD),        # 16_UNORM; float32 narrowed: 0x3BFE
    (7, 0xFFEF, 0x3BFF),        # 16_UNORM; float32 narrowed: 0x3C00
    (8, 0x7FF7, 0x3BFF),        # 16_SNORM; float32 narrowed: 0x3C00
    (8, 0x8009, 0xBBFF),        # 16_SNORM; float32 narrowed: 0xBC00
]


@pytest.mark.parametrize("fmt, field, half", ONE_ROUNDING_SPLITS + [(8, 0x8000, 0xBC00), (8, 0x8001, 0xBC00)])
def test_normalized_loads_round_the_exact_quotient_once(fmt, field, half):
    wave = run(typed("buffer_load_format_d16_x"), struct.pack("<H", field), records=2, desc3=word3(fmt, 4))
    assert wave.v[8][0] == FILL & 0xFFFF0000 | half
    single = formats.decode(formats.format_for(fmt, False, True), struct.pack("<H", field))[0]
    if fmt == 7 or field not in (0x8000, 0x8001):
        assert d16.half_from_float(single) != half     # the float32 path would be one ulp off


def test_normalized_values_of_several_widths():
    # 16_16_16_16_UNORM: 0, 65535 → 1.0, 13107 → 0.2, 65519 → 0x3BFF (65519 / 65535 lies just below the
    # midpoint between 0x3BFF and 0x3C00; its float32 0x3F7FF000 is exactly on it).
    data = struct.pack("<4H", 0, 65535, 13107, 65519)
    wave = run(typed("buffer_load_format_d16_xyzw"), data, records=8, desc3=word3(65))
    assert lane0(wave) == [0x3C00 << 16, 0x3BFF << 16 | 0x3266]
    # 8_8_8_8_UNORM: 51 → 0.2 (0x3266), 128 → 0.50196 = 0x3804 (0.5 + 4 × 2^-11), 255 → 1.0, 0.
    wave = run(typed("buffer_load_format_d16_xyzw"), bytes([51, 128, 255, 0]), records=4, desc3=word3(56))
    assert lane0(wave) == [0x3804 << 16 | 0x3266, 0x3C00]
    # 16_UNORM 1 → 1/65535, a float16 subnormal: 2^24 / 65535 = 256.0039 units of 2^-24 → 0x0100.
    wave = run(typed("buffer_load_format_d16_x"), struct.pack("<H", 1), records=2, desc3=word3(7, 4))
    assert wave.v[8][0] & 0xFFFF == 0x0100


def half_fraction(half):
    """Exact value of a finite float16."""
    exponent, mantissa = (half >> 10) & 0x1F, half & 0x3FF
    magnitude = Fraction(mantissa, 1 << 24) if exponent == 0 else Fraction(1024 + mantissa, 1 << 10) * Fraction(2) ** (exponent - 15)
    return -magnitude if half & 0x8000 else magnitude


def nearest_half(value):
    """Independent oracle: struct's binary16 rounding of the nearest double, then the exactly nearest of
    that half and its two neighbours, ties to the even mantissa."""
    guess = struct.unpack("<H", struct.pack("<e", float(value)))[0]
    candidates = [h for h in (guess - 1, guess, guess + 1) if 0 <= h & 0x7FFF <= 0x7BFF and h >> 15 == guess >> 15]
    return min(candidates, key=lambda h: (abs(half_fraction(h) - value), h & 1))


def test_every_normalized_field_is_its_exact_quotient_rounded_once():
    differs = {}
    for width in (2, 4, 5, 6, 8, 10, 11, 16):
        unorm = formats.Format(0, "UNORM", formats.UNORM, (width,), 4, 4)
        snorm = formats.Format(0, "SNORM", formats.SNORM, (width,), 4, 4)
        largest = (1 << (width - 1)) - 1
        for field in range(1 << width):
            number = field - (1 << width) if field >> (width - 1) else field
            for fmt, exact in ((unorm, Fraction(field, (1 << width) - 1)),
                               (snorm, max(Fraction(number, largest), Fraction(-1)))):
                [single] = d16.decode(fmt, field.to_bytes(4, "little"))
                half = d16.narrow(fmt, single)
                assert half == nearest_half(exact), (fmt.name, width, field)
                narrowed = d16.narrow(fmt, formats.decode(fmt, field.to_bytes(4, "little"))[0])
                differs[fmt.name, width] = differs.get((fmt.name, width), 0) + (narrowed != half)
    # Only two UNORM16 and two SNORM16 values separate the two roundings (ONE_ROUNDING_SPLITS).
    assert {key: count for key, count in differs.items() if count} == {("UNORM", 16): 2, ("SNORM", 16): 2}


@pytest.mark.parametrize("fmt, data, halves", [
    (9, struct.pack("<H", 2049), [0x6800]),          # 16_USCALED: 2049 → 2048 (halfway, to even)
    (9, struct.pack("<H", 2051), [0x6802]),          # 2051 → 2052
    (9, struct.pack("<H", 65535), [INFINITY]),       # past 65520: infinity (assumed)
    (10, struct.pack("<H", 0x8000), [0xF800]),       # 16_SSCALED: -32768
    (4, b"\x80", [0xD800]),                          # 8_SSCALED: -128.0
    (6, b"\x80", [0xFF80]),                          # 8_SINT: -128 sign-extended to 16 bits
    (5, b"\xff", [0x00FF]),                          # 8_UINT: 255
    (12, b"\x00\x80", [0x8000]),                     # 16_SINT: -32768
    (11, b"\xff\xff", [0xFFFF]),                     # 16_UINT: 65535
    (13, struct.pack("<H", 0x7C01), [0x7C01]),       # 16_FLOAT: unchanged, signalling NaN included (assumed)
    (13, struct.pack("<H", 0x0001), [0x0001]),       # 16_FLOAT subnormal
    (29, struct.pack("<2H", 0x3C00, 0xC000), [0x3C00, 0xC000]),
    (49, struct.pack("<I", 3 | 0x200 << 2 | 5 << 12 | 0x3FF << 22), [0xFFFF, 0xFE00, 5, 0xFFFF]),  # 10_10_10_2_SINT
])
def test_scaled_integer_and_half_formats(fmt, data, halves):
    count = len(halves)
    name = "buffer_load_format_d16_" + "xyzw"[:count]
    selects = sum((4 + i) << (3 * i) for i in range(count))
    wave = run(typed(name), data, records=4, desc3=word3(fmt, selects))
    expected = [(halves[i] | (halves[i + 1] << 16 if i + 1 < count else 0xDEAD0000)) for i in range(0, count, 2)]
    assert lane0(wave, count=len(expected)) == expected


# --- Packing, DST_SEL, EXEC, MTBUF -------------------------------------------------------

def test_halves_are_packed_and_unused_halves_kept():
    data = struct.pack("<4H", 1, 2, 3, 4)
    desc3 = word3(69)                                                    # 16_16_16_16_UINT
    assert lane0(run(typed("buffer_load_format_d16_x"), data, records=8, desc3=desc3)) == [0xDEAD0001, FILL]
    assert lane0(run(typed("buffer_load_format_d16_xy"), data, records=8, desc3=desc3)) == [0x00020001, FILL]
    assert lane0(run(typed("buffer_load_format_d16_xyz"), data, records=8, desc3=desc3)) == [0x00020001, 0xDEAD0003]
    assert lane0(run(typed("buffer_load_format_d16_xyzw"), data, records=8, desc3=desc3)) == [0x00020001, 0x00040003]
    # _d16_hi_x: bits 31-16, bits 15-0 kept.
    assert lane0(run(typed("buffer_load_format_d16_hi_x"), data, records=8, desc3=desc3)) == [0x0001BEEF, FILL]


def test_dst_sel_one_is_one_in_sixteen_bits_and_out_of_range_reads_zero():
    # DST_SEL W, 1, X, 0. 8_8_8_8_UNORM: 1 is 1.0 = 0x3C00; 8_8_8_8_UINT: the integer 1.
    selects = 7 | 1 << 3 | 4 << 6 | 0 << 9
    data = bytes([255, 0, 0, 51])
    wave = run(typed("buffer_load_format_d16_xyzw"), data, records=4, desc3=word3(56, selects))
    assert lane0(wave) == [ONE_HALF << 16 | 0x3266, 0x3C00]
    wave = run(typed("buffer_load_format_d16_xyzw"), data, records=4, desc3=word3(60, selects))
    assert lane0(wave) == [1 << 16 | 51, 255]
    # Out of the buffer (records = 0): components 0, DST_SEL 1 still 1.0.
    wave = run(typed("buffer_load_format_d16_xyzw"), data, records=0, desc3=word3(56, selects))
    assert lane0(wave) == [ONE_HALF << 16, 0]


def test_inactive_lanes_keep_their_registers():
    # EXEC = lanes 0-3 and 31; index = lane, 16_16_SINT, element i = (-i, i).
    data = b"".join(struct.pack("<2h", -i, i) for i in range(32))
    setup = [sop1("s_mov_b32", EXEC_LO, LITERAL), 0x8000000F]
    wave = run(typed("buffer_load_format_d16_xy", vaddr=0, idxen=1), data, records=32, stride=4,
               desc3=word3(28, 4 | 5 << 3), setup=setup)
    for lane in range(32):
        expected = (lane << 16 | (-lane & 0xFFFF)) if lane < 4 or lane == 31 else FILL
        assert wave.v[8][lane] == expected, lane


def test_tbuffer_d16_uses_the_instruction_format():
    # tbuffer 16_16_16_16_FLOAT (71); V# 32_FLOAT, DST_SEL zero: no effect on MTBUF.
    data = struct.pack("<4H", 0x3C00, 0xC000, 0x7BFF, 0x8001)
    wave = run(typed("tbuffer_load_format_d16_xyzw", fmt=71), data, records=8, desc3=word3(22, 0))
    assert lane0(wave) == [0xC0003C00, 0x80017BFF]
    # tbuffer 32_32_32_FLOAT (74), xyz: 0.5, 65520 (toward zero: 65504), -0.0; high half of v9 kept.
    data = floats(0x3F000000, 0x477FF000, 0x80000000)
    wave = run(typed("tbuffer_load_format_d16_xyz", fmt=74), data, records=12, desc3=word3(22, 0))
    assert lane0(wave) == [0x7BFF << 16 | 0x3800, 0xDEAD8000]
    # V# FORMAT INVALID: access disabled, zeros written in the halves.
    wave = run(typed("tbuffer_load_format_d16_xy", fmt=29), data, records=12, desc3=0)
    assert lane0(wave) == [0, FILL]


# --- Stores ------------------------------------------------------------------------------

def store(name, value_words, fmt_code, records=16, typed_fmt=None, selects=XYZW, before=b"\xa5" * 16):
    setup = [word for i, value in enumerate(value_words) for word in (vop1("v_mov_b32", 8 + i, LITERAL), value)]
    options = {"fmt": typed_fmt} if typed_fmt is not None else {}
    wave = run(typed(name, **options), before, records=records, desc3=word3(fmt_code, selects), setup=setup)
    return wave.memory.read(BUFFER, 16)


def test_half_stores_to_float_formats():
    # 16_16_16_16_FLOAT: the halves unchanged (signalling NaN and subnormal included).
    written = store("buffer_store_format_d16_xyzw", [0x7C01_3C00, 0x0001_C000], 71)
    assert written[:8] == struct.pack("<4H", 0x3C00, 0x7C01, 0xC000, 0x0001)
    # 32_32_FLOAT: exact widening; 1.0, NaN 0x7C01 → 0x7F802000 (payload << 13, as #150 widens 16_FLOAT).
    written = store("buffer_store_format_d16_xy", [0x7C01_3C00], 64, selects=4 | 5 << 3)
    assert written[:8] == floats(0x3F800000, 0x7F802000)
    # 32_FLOAT from the high half (_d16_hi_x): 2^-24 → 0x33800000.
    written = store("buffer_store_format_d16_hi_x", [0x0001_4000], 22, selects=4)
    assert written[:4] == floats(0x33800000)


def test_half_stores_to_integer_formats():
    # 8_8_8_8_SINT: halves -128 (0xFF80), 256 → 127, -1, 0x8000 = -32768 → -128.
    written = store("buffer_store_format_d16_xyzw", [0x0100_FF80, 0x8000_FFFF], 61)
    assert written[:4] == bytes([0x80, 0x7F, 0xFF, 0x80])
    # 8_8_8_8_UINT: zero-extended then saturated: 0xFF80 → 255, 7, 0, 0x8000 → 255.
    written = store("buffer_store_format_d16_xyzw", [0x0007_FF80, 0x8000_0000], 60)
    assert written[:4] == bytes([0xFF, 0x07, 0x00, 0xFF])
    # 32_SINT and 32_UINT: 0x8000 sign- or zero-extended.
    assert store("buffer_store_format_d16_x", [0x1234_8000], 21, selects=4)[:4] == floats(0xFFFF8000)
    assert store("buffer_store_format_d16_x", [0x1234_8000], 20, selects=4)[:4] == floats(0x00008000)
    # tbuffer 16_16_16_16_UINT, d16_xyzw = identity; tbuffer 32_32_32_SINT xyz: z from bits 15-0 of v9.
    written = store("tbuffer_store_format_d16_xyzw", [0x0002_0001, 0x0004_0003], 22, typed_fmt=69, selects=0)
    assert written[:8] == struct.pack("<4H", 1, 2, 3, 4)
    written = store("tbuffer_store_format_d16_xyz", [0xFFFF_0005, 0x7777_FFFE], 22, typed_fmt=73, selects=0)
    assert written[:12] == floats(5, 0xFFFFFFFF, 0xFFFFFFFE)


@pytest.mark.parametrize("name, options, desc3, message", [
    ("buffer_load_format_d16_x", {}, word3(20, 4), "d16 load with format 32_UINT"),
    ("tbuffer_load_format_d16_xy", {"fmt": 63}, word3(22, 0), "d16 load with format 32_32_SINT"),
    ("buffer_store_format_d16_x", {}, word3(1, 4), "d16 store with format 8_UNORM"),
    ("tbuffer_store_format_d16_xy", {"fmt": 25}, word3(22, 0), "d16 store with format 16_16_USCALED"),
    ("buffer_load_format_d16_x", {}, word3(36, 4), "10_11_11_FLOAT"),
    ("buffer_load_format_d16_xy", {}, word3(22, 4 | 5 << 3), "DST_SEL 5"),
    ("buffer_store_format_d16_xy", {}, word3(64, XYZW), "DST_SEL"),
    ("tbuffer_load_format_d16_xyzw", {"fmt": 64}, word3(22), "4 composantes"),
    ("buffer_load_format_d16_x", {"tfe": 1}, word3(22, 4), "tfe"),
    ("buffer_load_format_d16_x", {"offset": 2}, word3(22, 4), "non alignée"),
])
def test_unsupported_d16_forms_are_named(name, options, desc3, message):
    with pytest.raises(rdna2_exec.Untranslated, match=message):
        run(typed(name, **options), bytes(16), records=16, desc3=desc3)

