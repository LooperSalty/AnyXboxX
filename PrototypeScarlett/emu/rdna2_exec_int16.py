"""16-bit integer VALU of the RDNA 2 reference interpreter (VOP3 and packed VOP3P).

Covered: the VOP3-only 16-bit ops (v_add/sub_nc_{u16,i16}, v_mul_lo_u16, v_lshlrev_b16, v_lshrrev_b16,
v_ashrrev_i16, v_max/min_{u16,i16}, v_mad_{u16,i16}, v_min3/max3/med3_{u16,i16}), the 16 x 16 + 32 multiply-adds
v_mad_u32_u16 and v_mad_i32_i16, the saturating packs v_cvt_pk_u16_u32 and v_cvt_pk_i16_i32, the signed 32-bit
v_add_nc_i32 and v_sub_nc_i32, and the 14 packed integer ops v_pk_{add,sub,mad}_{u16,i16}, v_pk_mul_lo_u16,
v_pk_{max,min}_{u16,i16}, v_pk_lshlrev_b16, v_pk_lshrrev_b16 and v_pk_ashrrev_i16.

Semantics:
- 16-bit ops (VOP3): op_sel[i] reads S_i[31:16] instead of S_i[15:0], op_sel[3] writes D[31:16] instead of
  D[15:0], and the other half of D is kept. Each result wraps to 16 bits; with clamp, add/sub/mad saturate the
  exact value to [0, 0xFFFF] (u16) or [-32768, 32767] (i16).
- v_mad_u32_u16 / v_mad_i32_i16: D = S0.half * S1.half + S2 (halves zero- or sign-extended), op_sel[0-1] pick
  the source halves; clamp saturates the 32-bit sum to the type's range. op_sel[2] and op_sel[3] (on the 32-bit
  S2 and D) are refused.
- v_cvt_pk_u16_u32 / v_cvt_pk_i16_i32: D = {sat16(S1), sat16(S0)}.
- v_add_nc_i32 / v_sub_nc_i32: 32-bit wrap; clamp saturates to [0x80000000, 0x7FFFFFFF].
- Packed ops (VOP3P): result half 0 takes each source's half from op_sel[i], half 1 from op_sel_hi[i]; each
  half is computed alone and wraps; clamp saturates add/sub/mad per half and leaves max/min unchanged.
- A constant source is its 32-bit value split like a register: integers sign-extended to 32 bits, inline
  floats their float32 bits (1.0 = 0x3F800000: low half 0, high half 0x3F80), a literal its whole dword.

Sources (labelled as in docs/research/rdna2-sources.md; printed page numbers of the November 2020 PDF):
- AMD, "RDNA 2" Instruction Set Architecture: section 12.10 VOP3P opcodes 0-13 (p. 162-163) and section 12.12
  VOP3 opcodes 771-788, 832, 850-857, 862, 874-875, 883, 885, 886, 895 (p. 178-187): the per-op pseudocode,
  "Supports saturation" on the 16-bit add/sub/mad and v_add/sub_nc_i32, and for v_mad_u16/v_mad_i16 "If
  op_sel[3] is 1: Result is written to 16 MSBs of destination VGPR and lo 16 bits are preserved". Section 13.3,
  tables 82 and 86 (p. 258, 263): VOP3 OPSEL "Operand select for 16-bit data. 0 = select low half, 1 = select
  high half", [11]-[13] the sources and [14] the destination; VOP3P OPSEL / OPSEL_HI for the low and high
  results. Section 6.2.5, table 21 (p. 44): the opcodes usable with OPSEL. Section 6.5 (p. 48): for integer
  operations clamp "clamps the result to the largest and smallest representable value" (so it cannot change a
  max or a min, whose result is one of its in-range inputs).
- LLVM 20.1.8 (cross-check): lib/Target/AMDGPU/GCNSubtarget.cpp zeroesHigh16BitsOfDest, "On gfx10, all 16-bit
  instructions preserve the high bits" (the kept half of D); test/CodeGen/AMDGPU uaddsat.ll, usubsat.ll,
  saddsat.ll and ssubsat.ll select the saturating i16, v2i16 and signed i32 add/sub to v_add/sub_nc_{u16,i16},
  v_pk_add/sub_{u16,i16} and v_add/sub_nc_i32 with clamp on GFX10 (the PDF calls the v_sub_nc_i16 saturation
  "unsigned 16-bit"; LLVM's ssubsat makes it signed, which is what this module does);
  lib/Target/AMDGPU/Utils/AMDGPUBaseInfo.cpp getInlineEncodingV216: for (packed) 16-bit instructions "integer
  encodings (-16 .. 64) are always produced as sign-extended 32-bit values" and float encodings "for UI16
  instructions: corresponding single-precision float value"; test/CodeGen/AMDGPU/add.v2i16.ll (GFX10) encodes
  <123, 456> as the literal 0x1c8007b and <0, 16256> as the inline constant 1.0.
- Mesa (MIT, cross-check): nir_opcodes.py pack_uint_2x16 / pack_sint_2x16 "Convert two (un)signed integers into a
  packed (un)signed short (clamp is applied)", which ACO (aco_select_nir_alu.cpp) emits as v_cvt_pk_u16_u32 /
  v_cvt_pk_i16_i32; ACO also uses v_cvt_pk_i16_i32 to "Convert the exponent to 16bit int with saturation".
- Assumed, to be measured (akimfomin/PortBridge#84): the packs saturate (Mesa/ACO above); a 32-bit literal read
  through op_sel on a VOP3 16-bit op gives its high half, as the packed form does in LLVM's add.v2i16.ll.

Refused by name (Untranslated): VOP3 abs/neg/omod and VOP3P neg_lo/neg_hi (float modifiers, no integer
meaning); clamp where it is not implemented (v_mul_lo_u16 and v_pk_mul_lo_u16, whose "result" before the clamp
the sources above do not settle, the 16-bit shifts and min/max, min3/max3/med3, the packs, the packed shifts);
op_sel[2]/op_sel[3] on v_mad_u32_u16 / v_mad_i32_i16 (select bits on a 32-bit operand, not defined); op_sel on
the packs and on v_add/sub_nc_i32 (32-bit sources). The select bits of the third source of a two-source op are
ignored: no third operand is read (LLVM writes op_sel_hi[2] = 0 there).
"""
from emu.rdna2_exec_ops import INT32_MAX, INT32_MIN, MASK32, signed
from emu.rdna2_operands_vop3p import (CLAMP_BIT, NEG_HI_SHIFT, NEG_LO_SHIFT, OP_SEL_HI_SHIFTS, OP_SEL_SHIFT,
                                      SOURCE_MASK, SOURCE_SHIFTS, VDST_MASK)

HALF_MASK, HALF_BITS, HIGH_HALF = 0xFFFF, 16, 0xFFFF_0000
U16 = (0, 0xFFFF)
I16 = (-0x8000, 0x7FFF)
U32 = (0, MASK32)
I32 = (INT32_MIN, INT32_MAX)
SHIFT_MASK = 15
DESTINATION_HIGH = 1 << 3                   # op_sel[3]
WIDE_SELECTS = 0b1100                       # op_sel[2] and op_sel[3] of v_mad_u32_u16 / v_mad_i32_i16
THREE_BITS = 0x7
SATURATE, NO_OP = "saturate", "no-op"       # what clamp does; absent: refused


def s16(value):
    return signed(value & HALF_MASK, HALF_BITS)


def _median(a, b, c):
    return sorted((a, b, c))[1]


# Operation on 16-bit halves (zero-extended): (function → exact integer, sources, saturation range).
HALF_OPERATIONS = {
    "add_u16": (lambda a, b, c: a + b, 2, U16),
    "sub_u16": (lambda a, b, c: a - b, 2, U16),
    "add_i16": (lambda a, b, c: s16(a) + s16(b), 2, I16),
    "sub_i16": (lambda a, b, c: s16(a) - s16(b), 2, I16),
    "mul_lo_u16": (lambda a, b, c: a * b, 2, U16),
    "lshlrev_b16": (lambda a, b, c: b << (a & SHIFT_MASK), 2, None),
    "lshrrev_b16": (lambda a, b, c: b >> (a & SHIFT_MASK), 2, None),
    "ashrrev_i16": (lambda a, b, c: s16(b) >> (a & SHIFT_MASK), 2, None),
    "max_u16": (lambda a, b, c: max(a, b), 2, U16),
    "max_i16": (lambda a, b, c: max(s16(a), s16(b)), 2, I16),
    "min_u16": (lambda a, b, c: min(a, b), 2, U16),
    "min_i16": (lambda a, b, c: min(s16(a), s16(b)), 2, I16),
    "mad_u16": (lambda a, b, c: a * b + c, 3, U16),
    "mad_i16": (lambda a, b, c: s16(a) * s16(b) + s16(c), 3, I16),
    "min3_u16": (lambda a, b, c: min(a, b, c), 3, U16),
    "min3_i16": (lambda a, b, c: min(s16(a), s16(b), s16(c)), 3, I16),
    "max3_u16": (lambda a, b, c: max(a, b, c), 3, U16),
    "max3_i16": (lambda a, b, c: max(s16(a), s16(b), s16(c)), 3, I16),
    "med3_u16": (lambda a, b, c: _median(a, b, c), 3, U16),
    "med3_i16": (lambda a, b, c: _median(s16(a), s16(b), s16(c)), 3, I16),
}
# VOP3 mnemonic: (half operation, what clamp does)
VOP3_HALF = {
    "v_add_nc_u16": ("add_u16", SATURATE), "v_sub_nc_u16": ("sub_u16", SATURATE),
    "v_add_nc_i16": ("add_i16", SATURATE), "v_sub_nc_i16": ("sub_i16", SATURATE),
    "v_mad_u16": ("mad_u16", SATURATE), "v_mad_i16": ("mad_i16", SATURATE),
    **{f"v_{operation}": (operation, None) for operation in (
        "mul_lo_u16", "lshlrev_b16", "lshrrev_b16", "ashrrev_i16", "max_u16", "max_i16", "min_u16", "min_i16",
        "min3_u16", "min3_i16", "max3_u16", "max3_i16", "med3_u16", "med3_i16")},
}
# VOP3P mnemonic: (half operation, what clamp does)
PACKED = {
    **{f"v_pk_{operation}": (operation, SATURATE) for operation in (
        "add_u16", "sub_u16", "add_i16", "sub_i16", "mad_u16", "mad_i16")},
    **{f"v_pk_{operation}": (operation, NO_OP) for operation in ("max_u16", "max_i16", "min_u16", "min_i16")},
    **{f"v_pk_{operation}": (operation, None) for operation in (
        "mul_lo_u16", "lshlrev_b16", "lshrrev_b16", "ashrrev_i16")},
}
MAD_32_16 = {"v_mad_u32_u16": False, "v_mad_i32_i16": True}       # name: signed
PACK_32 = {"v_cvt_pk_u16_u32": U16, "v_cvt_pk_i16_i32": I16}
ADD_SUB_I32 = {"v_add_nc_i32": 1, "v_sub_nc_i32": -1}
VOP3 = frozenset(VOP3_HALF) | frozenset(MAD_32_16) | frozenset(PACK_32) | frozenset(ADD_SUB_I32)


def half_result(operation, halves, clamp):
    """16-bit result of a half operation on zero-extended halves; `clamp`: saturate to the type's range."""
    function, _, bounds = HALF_OPERATIONS[operation]
    value = function(*halves)
    if clamp and bounds is not None:
        value = min(max(value, bounds[0]), bounds[1])
    return value & HALF_MASK


def _half(value, high):
    return value >> HALF_BITS & HALF_MASK if high else value & HALF_MASK


def vop3_refusal(name, fields):
    """Reason not to execute (or translate) this VOP3 form, or None; `fields`: emu.rdna2_exec_vop3.Fields."""
    present = [label for label, value in (("abs", fields.abs), ("neg", fields.neg), ("omod", fields.omod))
               if value]
    if present:
        return f"{name} with {', '.join(present)}: float modifiers on integer sources, not defined"
    if name in PACK_32 or name in ADD_SUB_I32:
        if fields.op_sel:
            return f"{name} with op_sel: 32-bit sources, not defined"
        if fields.clamp and name in PACK_32:
            return f"{name} with clamp: not implemented"
    if name in MAD_32_16 and fields.op_sel & WIDE_SELECTS:
        return f"{name} with op_sel[2]/op_sel[3]: select bits on a 32-bit operand, not defined"
    if name in VOP3_HALF and fields.clamp and VOP3_HALF[name][1] != SATURATE:
        return f"{name} with clamp: not implemented"
    return None


def packed_refusal(name, word):
    """Reason not to execute (or translate) this VOP3P form, or None; `word`: the two instruction words."""
    present = [label for label, shift in (("neg_lo", NEG_LO_SHIFT), ("neg_hi", NEG_HI_SHIFT))
               if word >> shift & THREE_BITS]
    if present:
        return f"{name} with {', '.join(present)}: float modifiers on integer sources, not defined"
    if word >> CLAMP_BIT & 1 and PACKED[name][1] is None:
        return f"{name} with clamp: not implemented"
    return None


def _refuse(message):
    from emu.rdna2_exec import Untranslated
    raise Untranslated(message)


def execute_vop3(wave, name, fields, literal):
    """Execute a VOP3 instruction of VOP3; `fields`: emu.rdna2_exec_vop3.Fields."""
    message = vop3_refusal(name, fields)
    if message:
        _refuse(message)
    count = 3 if name in MAD_32_16 or name in VOP3_HALF and HALF_OPERATIONS[VOP3_HALF[name][0]][1] == 3 else 2
    destination = wave.v[fields.vdst]
    for lane in wave.active():
        values = [wave.read_lane(code, lane, literal) for code in fields.sources[:count]]
        destination[lane] = _vop3_lane(name, fields, values, destination[lane])


def _vop3_lane(name, fields, values, old):
    op_sel, clamp = fields.op_sel, fields.clamp
    if name in VOP3_HALF:
        operation = VOP3_HALF[name][0]
        halves = [_half(value, op_sel >> i & 1) for i, value in enumerate(values)] + [0] * (3 - len(values))
        result = half_result(operation, halves, clamp)
        if op_sel & DESTINATION_HIGH:
            return old & HALF_MASK | result << HALF_BITS
        return old & HIGH_HALF | result
    if name in MAD_32_16:
        is_signed = MAD_32_16[name]
        a, b = (_half(value, op_sel >> i & 1) for i, value in enumerate(values[:2]))
        product = s16(a) * s16(b) if is_signed else a * b
        total = product + (signed(values[2]) if is_signed else values[2])
        if clamp:
            low, high = I32 if is_signed else U32
            total = min(max(total, low), high)
        return total & MASK32
    if name in PACK_32:
        low, high = PACK_32[name]
        read = signed if name == "v_cvt_pk_i16_i32" else int
        a, b = (min(max(read(value), low), high) & HALF_MASK for value in values)
        return a | b << HALF_BITS
    total = signed(values[0]) + ADD_SUB_I32[name] * signed(values[1])
    return (min(max(total, INT32_MIN), INT32_MAX) if clamp else total) & MASK32


def execute_packed(wave, name, word, literal):
    """Execute a packed VOP3P instruction of PACKED; `word`: the two instruction words (64-bit integer)."""
    message = packed_refusal(name, word)
    if message:
        _refuse(message)
    operation, clamp_effect = PACKED[name]
    count = HALF_OPERATIONS[operation][1]
    clamp = bool(word >> CLAMP_BIT & 1) and clamp_effect == SATURATE
    codes = [word >> shift & SOURCE_MASK for shift in SOURCE_SHIFTS[:count]]
    low_select = [word >> (OP_SEL_SHIFT + i) & 1 for i in range(count)]
    high_select = [word >> shift & 1 for shift in OP_SEL_HI_SHIFTS[:count]]
    destination = wave.v[word & VDST_MASK]
    for lane in wave.active():
        values = [wave.read_lane(code, lane, literal) for code in codes]
        padding = [0] * (3 - count)
        low = half_result(operation, [_half(v, s) for v, s in zip(values, low_select)] + padding, clamp)
        high = half_result(operation, [_half(v, s) for v, s in zip(values, high_select)] + padding, clamp)
        destination[lane] = low | high << HALF_BITS
