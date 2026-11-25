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
- Every other format (UNORM, SNORM, USCALED, SSCALED, FLOAT): float32 → float16. Assumed: IEEE 754
  round to nearest even, overflow to infinity, float16 subnormals kept (the measured rule of #150 rounds
  to nearest; nothing documents the narrowing). Narrowing the float32 value is not the same as rounding
  the exact UNORM/SNORM quotient once: 16_UNORM 65455 and 65519 and 16_SNORM 32759 differ by one ulp
  (tests/test_rdna2_exec_format_d16.py). A float16 field comes back unchanged (widened exactly, then
  narrowed exactly). A float32 NaN keeps its sign and the top 10 bits of its payload, the inverse of
  the widening measured in #150 (assumed); a NaN whose top 10 payload bits are zero would turn into
  infinity and is refused by name.
Stores: each 16-bit half is widened to the 32-bit value the non-d16 store would take, then written with
its rules (rdna2_buffer_format.encode): UINT zero-extends, SINT sign-extends (section 8.1.3: integers of
the storage type), float16 → float32 is exact for 32-bit FLOAT formats, a 16-bit FLOAT field takes the
half unchanged. UNORM, SNORM and SCALED stores stay refused, as in #150.
"""
from emu import rdna2_buffer_format as formats
from emu.rdna2_exec_ops import MASK32, signed

HALF_BITS, HALF_MASK = 16, 0xFFFF
HALF_SIGN, HALF_INFINITY = 0x8000, 0x7C00
FLOAT_EXPONENT_MAX, FLOAT_MANTISSA_BITS = 0xFF, 23
HALF_PAYLOAD_SHIFT = 13                         # 23 - 10 bits of mantissa
HALF_NORMAL_BIAS = 112                          # 127 - 15
LOWEST_ROUNDED_EXPONENT = 101                   # below 2^-26: always rounds to zero


def slots(access):
    """(VGPR after VDATA, shift) of each component: two per VGPR, low half first; _d16_hi_x: high half."""
    if access.high:
        return [(0, HALF_BITS)]
    return [(i // 2, HALF_BITS * (i % 2)) for i in range(access.count)]


def _round_shift(value, drop):
    """value / 2^drop rounded to nearest, ties to even."""
    quotient, remainder, half = value >> drop, value & ((1 << drop) - 1), 1 << (drop - 1)
    if remainder > half or (remainder == half and quotient & 1):
        quotient += 1
    return quotient


def half_from_float(bits):
    """float32 bits → float16 bits, round to nearest even (assumed, see the module docstring)."""
    sign = (bits >> 16) & HALF_SIGN
    exponent, mantissa = (bits >> FLOAT_MANTISSA_BITS) & FLOAT_EXPONENT_MAX, bits & 0x7FFFFF
    if exponent == FLOAT_EXPONENT_MAX:
        payload = mantissa >> HALF_PAYLOAD_SHIFT
        if mantissa and not payload:
            raise formats.FormatError(f"d16: NaN 0x{bits:08X} whose top 10 payload bits are zero (narrowing not documented)")
        return sign | HALF_INFINITY | payload
    if exponent < LOWEST_ROUNDED_EXPONENT:
        return sign
    full = mantissa | 1 << FLOAT_MANTISSA_BITS
    biased = exponent - HALF_NORMAL_BIAS
    if biased >= 1:
        rounded = ((biased - 1) << 10) + _round_shift(full, HALF_PAYLOAD_SHIFT)
    else:                                       # float16 subnormal: units of 2^-24
        rounded = _round_shift(full, HALF_PAYLOAD_SHIFT + 1 - biased)
    return sign | min(rounded, HALF_INFINITY)


def narrow(fmt, value):
    """16-bit result of a d16 load from the 32-bit result of the same non-d16 load."""
    if fmt.kind in formats.INTEGER_KINDS:
        return value & HALF_MASK
    return half_from_float(value)


def widen(fmt, half, width):
    """32-bit value a non-d16 store would take for the 16-bit half of a d16 store (field of `width` bits)."""
    if fmt.kind == formats.SINT:
        return signed(half, HALF_BITS) & MASK32
    if fmt.kind == formats.FLOAT and width != HALF_BITS:
        return formats.half_bits(half)
    return half                                 # UINT, 16-bit FLOAT
