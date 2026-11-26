"""Packed f16, mixed-precision FMA, and f16 dot product (VOP3P and v_pk_fmac_f16) in the RDNA 2 interpreter.

Covers every remaining VOP3P instruction plus v_pk_fmac_f16 (VOP2 and VOP3), closing VOP3P at 100%:
- Packed f16 arithmetic (VOP3P): v_pk_fma_f16, v_pk_add_f16, v_pk_mul_f16, v_pk_min_f16, v_pk_max_f16.
- Packed f16 accumulate (VOP2 0x3C and VOP3 0x13C): v_pk_fmac_f16.
- Mixed-precision FMA (VOP3P): v_fma_mix_f32, v_fma_mixlo_f16, v_fma_mixhi_f16.
- Packed f16 dot product (VOP3P): v_dot2_f32_f16.

Sources:
- AMD, "RDNA 2" Instruction Set Architecture (November 2020): section 12.10 VOP3P opcodes 14-19 and 32-34
  (v_pk_fma/add/mul/min/max_f16, v_dot2_f32_f16, v_fma_mix_f32/lo/hi), section 12.7 VOP2 opcode 60
  (v_pk_fmac_f16), section 6.2.1 (inline float constants replicated across both 16-bit halves for packed f16),
  and section 13.3 table 86 (VOP3P OPSEL, OPSEL_HI, NEG_LO, NEG_HI, CLAMP).
- LLVM (cross-check): VOP3PInstructions.td and AMDGPUBaseInfo.cpp (getInlineEncodingV216, SelectVOP3PMadMixModsImpl):
  for v_fma_mix*, op_sel_hi[i] selects f16 (1) vs f32 (0), op_sel[i] picks the f16 half when op_sel_hi[i] == 1,
  neg_hi[i] is abs, and neg_lo[i] is neg; v_fma_mixlo_f16 / v_fma_mixhi_f16 preserve the untouched half of vdst.
- Assumed for v_dot2_f32_f16: evaluated as two exact f32 products summed and added to src2 in f32
  (`(a_lo * b_lo + a_hi * b_hi) + c`), matching the HLSL translation bit-for-bit.
"""
from emu.rdna2_exec_f16 import (EXTENSIONS, FLOAT_NAN, INLINE_HALVES, VGPR_FIRST, _is_nan, bits_half, half_bits,
                                half_clamp, half_max, half_min)
from emu.rdna2_exec_ops import MASK32, _float_op, _fma, bits_float, float_bits
from emu.rdna2_operands_vop3p import (CLAMP_BIT, NEG_HI_SHIFT, NEG_LO_SHIFT, OP_SEL_HI_SHIFTS, OP_SEL_SHIFT,
                                      SOURCE_MASK, SOURCE_SHIFTS, VDST_MASK)

HALF_MASK, HALF_BITS, HIGH_HALF = 0xFFFF, 16, 0xFFFF_0000
HALF_SIGN, FLOAT_SIGN, FLOAT_ABS, FLOAT_ONE = 0x8000, 0x8000_0000, 0x7FFF_FFFF, 0x3F80_0000
THREE_BITS = 0x7

H = bits_half
_f32_add = _float_op(lambda a, b: a + b)
_f32_mul = _float_op(lambda a, b: a * b)
_f32_fma = _float_op(_fma)

# Packed f16 VOP3P ops: name -> (source_count, fn(a, b, c) -> f16 bits).
PACKED_F16 = {
    "v_pk_fma_f16": (3, lambda a, b, c: half_bits(H(a) * H(b) + H(c))),
    "v_pk_add_f16": (2, lambda a, b, c: half_bits(H(a) + H(b))),
    "v_pk_mul_f16": (2, lambda a, b, c: half_bits(H(a) * H(b))),
    "v_pk_min_f16": (2, lambda a, b, c: half_min(a, b)),
    "v_pk_max_f16": (2, lambda a, b, c: half_max(a, b)),
}
FMA_MIX = frozenset({"v_fma_mix_f32", "v_fma_mixlo_f16", "v_fma_mixhi_f16"})
DOT2_F16 = frozenset({"v_dot2_f32_f16"})
VOP3P = frozenset(PACKED_F16) | FMA_MIX | DOT2_F16
PK_FMAC = frozenset({"v_pk_fmac_f16"})
ALL_OPS = VOP3P | PK_FMAC


def _refuse(message):
    from emu.rdna2_exec import Untranslated
    raise Untranslated(message)


def f16_to_f32_bits(h):
    return FLOAT_NAN if _is_nan(h) else float_bits(H(h))


def clamp_f32(bits):
    """Clamp f32 bits to [0.0, 1.0]: NaN and negatives (including -0.0) become +0.0."""
    if (bits & FLOAT_ABS) > 0x7F80_0000 or (bits & FLOAT_SIGN):
        return 0
    return min(bits, FLOAT_ONE)


def _read_pk_half(wave, code, lane, literal, high):
    """Read one 16-bit half for packed f16: inline float constants replicate to both halves."""
    if code in INLINE_HALVES:
        return INLINE_HALVES[code]
    word = wave.read_lane(code, lane, literal)
    return (word >> HALF_BITS) & HALF_MASK if high else word & HALF_MASK


def vop3p_refusal(name, word):
    """Reason not to execute or translate this VOP3P form, or None."""
    op_sel = [word >> (OP_SEL_SHIFT + i) & 1 for i in range(3)]
    op_sel_hi = [word >> shift & 1 for shift in OP_SEL_HI_SHIFTS]
    neg_hi = [word >> (NEG_HI_SHIFT + i) & 1 for i in range(3)]
    if name in FMA_MIX:
        for i in range(3):
            if not op_sel_hi[i] and op_sel[i]:
                return f"{name}: op_sel[{i}] on an f32 source (op_sel_hi[{i}]=0) is not defined"
    elif name in DOT2_F16:
        if op_sel[2] or neg_hi[2]:
            return f"{name}: op_sel[2] or neg_hi[2] on 32-bit f32 accumulator src2 is not defined"
    return None


def pk_fmac_vop2_refusal(name, fields):
    if fields["src0"] in EXTENSIONS:
        return f"{name}: SDWA/DPP form (src0 = 0x{fields['src0']:X}) not supported for packed f16"
    return None


def pk_fmac_vop3_refusal(name, fields):
    present = [label for label, value in (
        ("op_sel", fields.op_sel), ("abs", fields.abs), ("neg", fields.neg), ("omod", fields.omod)
    ) if value]
    if present:
        return f"{name} (VOP3) with {', '.join(present)}: not supported"
    return None


def execute_vop3p(wave, name, word, literal):
    """Execute a VOP3P instruction from VOP3P (`word`: 64-bit encoding)."""
    message = vop3p_refusal(name, word)
    if message:
        _refuse(message)
    clamp = bool(word >> CLAMP_BIT & 1)
    destination = wave.v[word & VDST_MASK]
    if name in PACKED_F16:
        count, operation = PACKED_F16[name]
        codes = [word >> shift & SOURCE_MASK for shift in SOURCE_SHIFTS[:count]]
        lo_sel = [word >> (OP_SEL_SHIFT + i) & 1 for i in range(count)]
        hi_sel = [word >> shift & 1 for shift in OP_SEL_HI_SHIFTS[:count]]
        neg_lo = [word >> (NEG_LO_SHIFT + i) & 1 for i in range(count)]
        neg_hi = [word >> (NEG_HI_SHIFT + i) & 1 for i in range(count)]
        for lane in wave.active():
            lo_args = [
                _read_pk_half(wave, code, lane, literal, sel) ^ (HALF_SIGN if neg else 0)
                for code, sel, neg in zip(codes, lo_sel, neg_lo)
            ] + [0] * (3 - count)
            hi_args = [
                _read_pk_half(wave, code, lane, literal, sel) ^ (HALF_SIGN if neg else 0)
                for code, sel, neg in zip(codes, hi_sel, neg_hi)
            ] + [0] * (3 - count)
            lo = operation(*lo_args)
            hi = operation(*hi_args)
            if clamp:
                lo, hi = half_clamp(lo), half_clamp(hi)
            destination[lane] = lo | (hi << HALF_BITS)
        return
    codes = [word >> shift & SOURCE_MASK for shift in SOURCE_SHIFTS]
    op_sel = [word >> (OP_SEL_SHIFT + i) & 1 for i in range(3)]
    op_sel_hi = [word >> shift & 1 for shift in OP_SEL_HI_SHIFTS]
    neg_lo = [word >> (NEG_LO_SHIFT + i) & 1 for i in range(3)]
    neg_hi = [word >> (NEG_HI_SHIFT + i) & 1 for i in range(3)]
    if name in FMA_MIX:
        for lane in wave.active():
            args = []
            for code, is_f16, hi_half, use_abs, use_neg in zip(codes, op_sel_hi, op_sel, neg_hi, neg_lo):
                val = f16_to_f32_bits(_read_pk_half(wave, code, lane, literal, hi_half)) if is_f16 else wave.read_lane(
                    code, lane, literal)
                if use_abs:
                    val &= FLOAT_ABS
                if use_neg:
                    val ^= FLOAT_SIGN
                args.append(val)
            res = _f32_fma(bits_float(args[0]), bits_float(args[1]), bits_float(args[2])) & MASK32
            if name == "v_fma_mix_f32":
                destination[lane] = clamp_f32(res) if clamp else res
            else:
                h = half_bits(bits_float(res))
                if clamp:
                    h = half_clamp(h)
                old = destination[lane]
                destination[lane] = (old & HALF_MASK) | (h << HALF_BITS) if name == "v_fma_mixhi_f16" else (
                    (old & HIGH_HALF) | h)
        return
    # v_dot2_f32_f16
    for lane in wave.active():
        a_lo = _read_pk_half(wave, codes[0], lane, literal, op_sel[0]) ^ (HALF_SIGN if neg_lo[0] else 0)
        a_hi = _read_pk_half(wave, codes[0], lane, literal, op_sel_hi[0]) ^ (HALF_SIGN if neg_hi[0] else 0)
        b_lo = _read_pk_half(wave, codes[1], lane, literal, op_sel[1]) ^ (HALF_SIGN if neg_lo[1] else 0)
        b_hi = _read_pk_half(wave, codes[1], lane, literal, op_sel_hi[1]) ^ (HALF_SIGN if neg_hi[1] else 0)
        c = wave.read_lane(codes[2], lane, literal) ^ (FLOAT_SIGN if neg_lo[2] else 0)
        p0 = _f32_mul(bits_float(f16_to_f32_bits(a_lo)), bits_float(f16_to_f32_bits(b_lo)))
        p1 = _f32_mul(bits_float(f16_to_f32_bits(a_hi)), bits_float(f16_to_f32_bits(b_hi)))
        res = _f32_add(bits_float(_f32_add(bits_float(p0), bits_float(p1))), bits_float(c)) & MASK32
        destination[lane] = clamp_f32(res) if clamp else res


def execute_pk_fmac_vop2(wave, name, fields, literal):
    """Execute VOP2 v_pk_fmac_f16."""
    message = pk_fmac_vop2_refusal(name, fields)
    if message:
        _refuse(message)
    src0, src1 = fields["src0"], VGPR_FIRST + fields["vsrc1"]
    destination = wave.v[fields["vdst"]]
    for lane in wave.active():
        old = destination[lane]
        a_lo, a_hi = _read_pk_half(wave, src0, lane, literal, False), _read_pk_half(wave, src0, lane, literal, True)
        b_lo, b_hi = _read_pk_half(wave, src1, lane, literal, False), _read_pk_half(wave, src1, lane, literal, True)
        lo = half_bits(H(a_lo) * H(b_lo) + H(old & HALF_MASK))
        hi = half_bits(H(a_hi) * H(b_hi) + H((old >> HALF_BITS) & HALF_MASK))
        destination[lane] = lo | (hi << HALF_BITS)


def execute_pk_fmac_vop3(wave, name, fields, literal):
    """Execute VOP3 v_pk_fmac_f16."""
    message = pk_fmac_vop3_refusal(name, fields)
    if message:
        _refuse(message)
    src0, src1 = fields.sources[0], fields.sources[1]
    destination = wave.v[fields.vdst]
    for lane in wave.active():
        old = destination[lane]
        a_lo, a_hi = _read_pk_half(wave, src0, lane, literal, False), _read_pk_half(wave, src0, lane, literal, True)
        b_lo, b_hi = _read_pk_half(wave, src1, lane, literal, False), _read_pk_half(wave, src1, lane, literal, True)
        lo = half_bits(H(a_lo) * H(b_lo) + H(old & HALF_MASK))
        hi = half_bits(H(a_hi) * H(b_hi) + H((old >> HALF_BITS) & HALF_MASK))
        if fields.clamp:
            lo, hi = half_clamp(lo), half_clamp(hi)
        destination[lane] = lo | (hi << HALF_BITS)
