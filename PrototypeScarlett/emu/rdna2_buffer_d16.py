"""16-bit typed buffer accesses (buffer_*_format_d16_*, tbuffer_*_format_d16_*): packing and conversions.

Shared by the interpreter (emu/rdna2_exec_format.py) and the HLSL translation (emu/rdna2_hlsl_format.py),
on top of the 32-bit conversions of emu/rdna2_buffer_format.py.

Packing (RDNA 2 is "packed d16": LLVM 20.1.8 lists FeatureUnpackedD16VMem only for GFX8, AMDGPU.td, and
declares the gfx10 tbuffer_load_format_d16_xy with one VGPR, BUFInstructions.td): component i sits in
VGPR VDATA + i / 2, low half first, then high half. A load writes only the halves it fills and keeps the
others: _d16_x and the third component of _d16_xyz write bits 15-0, _d16_hi_x writes bits 31-16. A store
reads the same halves.
- AMD, "RDNA 2" Instruction Set Architecture, November 2020: section 8.1.2 (p. 56, "The D16 instruction
  variants convert the results to packed 16-bit values. For example, BUFFER_LOAD_FORMAT_D16_XYZW writes
  two VGPRs"), section 8.1.4 "D16 Instructions" (p. 59, "a pair of data are stored in each 32-bit VGPR
  (LSBs first, then MSBs)"), section 8.1.6 (p. 64, "D16_HI loads data into or stores data from the upper
  16 bits of a VGPR"), section 12.14 (p. 211 and 215: BUFFER_LOAD_FORMAT_D16_X "D0[15:0] = MEM[ADDR]",
  BUFFER_LOAD_FORMAT_D16_HI_X "D0[31:16] = MEM[ADDR]").
- AMD machine-readable ISA (amdgpu_isa_rdna2.xml): BUFFER_LOAD_FORMAT_D16_X stores "into the low 16 bits",
  BUFFER_LOAD_FORMAT_D16_HI_X "into the high 16 bits of a 32-bit vector register".
- Assumed: the unused high half of the last VGPR of _d16_xyz is kept, like the high half of _d16_x.

Loads: section 8.1.4 says "data returned from the texture unit is converted to 16 bits", and section 8.1.3
(p. 58) that integer formats return integers of the storage type. So a d16 load takes the 32-bit result of
the non-d16 load (rdna2_buffer_format.decode, DST_SEL included) and narrows it:
- UINT and SINT of 16 bits or less: the low 16 bits (the zero- or sign-extended field fits). 32-bit
  integer formats are refused by name: whether they are truncated or clamped is not documented.
- UNORM and SNORM: the exact quotient c / (2^n - 1) (SNORM: c / (2^(n-1) - 1), -2^(n-1) giving -1.0)
  rounded ONCE to float16, to nearest even, float16 subnormals kept; not the float32 result narrowed.
  No public source settles this (the ISA only says the data "is converted to 16 bits"): assumed, to be
  measured in akimfomin/PortBridge#85. Only four values separate the two roundings: 16_UNORM 0xFFAF →
  0x3BFD and 0xFFEF → 0x3BFF, 16_SNORM 0x7FF7 → 0x3BFF and 0x8009 → 0xBBFF (narrowing the float32 gives
  0x3BFE, 0x3C00, 0x3C00, 0xBC00). The other widths (2, 4, 5, 6, 8, 10, 11 bits) give the same half under
  both roundings (tests/test_rdna2_exec_format_d16.py checks it).
- FLOAT: float32 → float16 toward zero, finite values past the range giving ±65504, float16 subnormals
  kept, infinities kept; a NaN keeps its sign and the top 10 bits of its payload, not quieted, or payload
  1 if those are all zero (0x7F800001 → 0x7C01). Assumed, to be measured (same request). A 16-bit FLOAT
  field comes back unchanged (widened exactly, then narrowed exactly, NaN payload included).
- USCALED, SSCALED: the float32 (the exact integer) → float16, round to nearest even, overflow to
  infinity (assumed).
Stores: each 16-bit half is widened to the 32-bit value the non-d16 store would take, then written with
its rules (rdna2_buffer_format.encode): UINT zero-extends, SINT sign-extends (section 8.1.3: integers of
the storage type), float16 → float32 is exact for 32-bit FLOAT formats (a NaN payload shifted left by 13, as
rdna2_buffer_format widens a 16-bit FLOAT field), a 16-bit FLOAT field takes the half unchanged. UNORM,
SNORM and SCALED stores stay refused, as in #150.
"""
from emu import rdna2_buffer_format as formats
from emu.rdna2_exec_ops import MASK32, signed

HALF_BITS, HALF_MASK = 16, 0xFFFF
HALF_SIGN, HALF_INFINITY, HALF_MINUS_ONE, HALF_LARGEST = 0x8000, 0x7C00, 0xBC00, 0x7BFF
HALF_SUBNORMAL_SHIFT = 14                       # quotients below 2^-14 are float16 subnormals
FLOAT_EXPONENT_MAX, FLOAT_MANTISSA_BITS = 0xFF, 23
HALF_PAYLOAD_SHIFT = 13                         # 23 - 10 bits of mantissa
HALF_NORMAL_BIAS = 112                          # 127 - 15
LOWEST_ROUNDED_EXPONENT = 101                   # below 2^-26: always rounds to zero


def slots(access):
    """(VGPR after VDATA, shift) of each component: two per VGPR, low half first; _d16_hi_x: high half."""
    if access.high:
        return [(0, HALF_BITS)]
    return [(i // 2, HALF_BITS * (i % 2)) for i in range(access.count)]


def _round_shift(value, drop, toward_zero=False):
    """value / 2^drop rounded to nearest, ties to even, or toward zero."""
    quotient, remainder, half = value >> drop, value & ((1 << drop) - 1), 1 << (drop - 1)
    if not toward_zero and (remainder > half or (remainder == half and quotient & 1)):
        quotient += 1
    return quotient


def half_from_float(bits, toward_zero=False):
    """float32 bits → float16 bits (rules and sources in the module docstring): round to nearest even with
    overflow to infinity, or `toward_zero` with finite values past the range giving ±65504."""
    sign = (bits >> 16) & HALF_SIGN
    exponent, mantissa = (bits >> FLOAT_MANTISSA_BITS) & FLOAT_EXPONENT_MAX, bits & 0x7FFFFF
    if exponent == FLOAT_EXPONENT_MAX:
        payload = mantissa >> HALF_PAYLOAD_SHIFT
        return sign | HALF_INFINITY | (1 if mantissa and not payload else payload)
    if exponent < LOWEST_ROUNDED_EXPONENT:
        return sign
    full = mantissa | 1 << FLOAT_MANTISSA_BITS
    biased = exponent - HALF_NORMAL_BIAS
    if biased >= 1:
        rounded = ((biased - 1) << 10) + _round_shift(full, HALF_PAYLOAD_SHIFT, toward_zero)
    else:                                       # float16 subnormal: units of 2^-24
        rounded = _round_shift(full, HALF_PAYLOAD_SHIFT + 1 - biased, toward_zero)
    return sign | min(rounded, HALF_LARGEST if toward_zero else HALF_INFINITY)


def half_from_ratio(numerator, denominator):
    """float16 bits of numerator / denominator (0 <= numerator <= denominator < 2^16) rounded once, to
    nearest even, float16 subnormals kept. Integer only: the HLSL BufHalfRatio does the same steps."""
    if numerator == 0:
        return 0
    shift = 0                                   # quotient in [2^-shift, 2^(1-shift))
    while numerator << shift < denominator:
        shift += 1
    exponent_shift = min(shift, HALF_SUBNORMAL_SHIFT)
    quotient, remainder = divmod(numerator << (exponent_shift + 10), denominator)
    if 2 * remainder > denominator or (2 * remainder == denominator and quotient & 1):
        quotient += 1
    # Normal: quotient in [1024, 2048] carries the implicit bit (2048 moves to the next exponent);
    # subnormal (shift > 14): units of 2^-24.
    return ((HALF_SUBNORMAL_SHIFT - exponent_shift) << 10) + quotient


def normalized_half(fmt, field, width):
    """float16 bits of a UNORM or SNORM field: the exact quotient rounded once (see the module docstring)."""
    if fmt.kind == formats.UNORM:
        return half_from_ratio(field, (1 << width) - 1)
    number, largest = signed(field, width), (1 << (width - 1)) - 1
    if number <= -largest:
        return HALF_MINUS_ONE
    magnitude = half_from_ratio(abs(number), largest)
    return magnitude | HALF_SIGN if number < 0 else magnitude


def decode(fmt, data):
    """32-bit components of a d16 load (rdna2_buffer_format.decode): a UNORM or SNORM component is the
    float32 of its float16, so that narrow() gives back that float16 exactly."""
    if fmt.kind not in (formats.UNORM, formats.SNORM):
        return formats.decode(fmt, data)
    return [formats.half_bits(normalized_half(fmt, field, width)) for field, width in formats.fields(fmt, data)]


def narrow(fmt, value):
    """16-bit result of a d16 load from the 32-bit result of the same non-d16 load."""
    if fmt.kind in formats.INTEGER_KINDS:
        return value & HALF_MASK
    return half_from_float(value, toward_zero=fmt.kind == formats.FLOAT)


def widen(fmt, half, width):
    """32-bit value a non-d16 store would take for the 16-bit half of a d16 store (field of `width` bits)."""
    if fmt.kind == formats.SINT:
        return signed(half, HALF_BITS) & MASK32
    if fmt.kind == formats.FLOAT and width != HALF_BITS:
        return formats.half_bits(half)
    return half                                 # UINT, 16-bit FLOAT
