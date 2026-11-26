"""HLSL translation of packed f16, mixed-precision FMA, v_dot2_f32_f16, and v_pk_fmac_f16.

Semantics and sources: emu/rdna2_exec_pkf16.py. Reuses the bit-exact f16 helpers in emu/rdna2_hlsl_f16.py
(F16Add, F16Mul, F16Fma, F16Min, F16Max, F16Clamp, F16ToF32, F32ToF16), FmaF32 in emu/rdna2_hlsl_fma.py,
and F32Add/F32Mul in emu/rdna2_hlsl_helpers.py. Pure per-lane expressions with no wave32-only intrinsics,
so wave32, wave64, and graphics stages share the exact same translation.
"""
from emu import rdna2_exec_pkf16 as pkf16
from emu.rdna2_exec_f16 import INLINE_HALVES, VGPR_FIRST
from emu.rdna2_hlsl_ops import Untranslatable, hexa, source
from emu.rdna2_operands_vop3p import (CLAMP_BIT, NEG_HI_SHIFT, NEG_LO_SHIFT, OP_SEL_HI_SHIFTS, OP_SEL_SHIFT,
                                      SOURCE_MASK, SOURCE_SHIFTS, VDST_MASK)

TRANSLATED = pkf16.ALL_OPS

HELPERS = """uint F32Clamp01(uint f) {
  if ((f & 0x7FFFFFFFu) > 0x7F800000u || (f & 0x80000000u) != 0u) return 0u;
  return min(f, 0x3F800000u);
}
"""

PACKED_TEMPLATES = {
    "v_pk_fma_f16": "F16Fma({a}, {b}, {c})",
    "v_pk_add_f16": "F16Add({a}, {b})",
    "v_pk_mul_f16": "F16Mul({a}, {b})",
    "v_pk_min_f16": "F16Min({a}, {b})",
    "v_pk_max_f16": "F16Max({a}, {b})",
}


def _pk_half_expr(var_or_const, code, high, neg=False):
    if code in INLINE_HALVES:
        val = INLINE_HALVES[code] ^ (0x8000 if neg else 0)
        return hexa(val)
    half = f"({var_or_const} >> 16)" if high else f"({var_or_const} & 0xFFFFu)"
    return f"({half} ^ 0x8000u)" if neg else half


def _src_bindings(codes, literal):
    return ", ".join(f"{var} = {source(code, literal)}" for var, code in zip("xyz", codes)
                     if code not in INLINE_HALVES)


def vop3p(name, word, literal, active):
    """HLSL lines for a VOP3P instruction in pkf16.VOP3P."""
    message = pkf16.vop3p_refusal(name, word)
    if message:
        raise Untranslatable(f"not translated to HLSL: {message}")
    d = word & VDST_MASK
    clamp = bool(word >> CLAMP_BIT & 1)
    if name in pkf16.PACKED_F16:
        count = pkf16.PACKED_F16[name][0]
        codes = [word >> shift & SOURCE_MASK for shift in SOURCE_SHIFTS[:count]]
        lo_sel = [word >> (OP_SEL_SHIFT + i) & 1 for i in range(count)]
        hi_sel = [word >> shift & 1 for shift in OP_SEL_HI_SHIFTS[:count]]
        neg_lo = [word >> (NEG_LO_SHIFT + i) & 1 for i in range(count)]
        neg_hi = [word >> (NEG_HI_SHIFT + i) & 1 for i in range(count)]
        bindings = _src_bindings(codes, literal)
        vars_names = ("x", "y", "z")[:count]
        lo_args = {k: _pk_half_expr(var, code, sel, neg)
                   for k, var, code, sel, neg in zip("abc", vars_names, codes, lo_sel, neg_lo)}
        hi_args = {k: _pk_half_expr(var, code, sel, neg)
                   for k, var, code, sel, neg in zip("abc", vars_names, codes, hi_sel, neg_hi)}
        lo_expr = PACKED_TEMPLATES[name].format(**lo_args)
        hi_expr = PACKED_TEMPLATES[name].format(**hi_args)
        if clamp:
            lo_expr, hi_expr = f"F16Clamp({lo_expr})", f"F16Clamp({hi_expr})"
        prefix = f"uint {bindings}; " if bindings else ""
        return [f"{{ {prefix}if ({active}) v[{d}] = ({lo_expr}) | (({hi_expr}) << 16); }}"]

    codes = [word >> shift & SOURCE_MASK for shift in SOURCE_SHIFTS]
    op_sel = [word >> (OP_SEL_SHIFT + i) & 1 for i in range(3)]
    op_sel_hi = [word >> shift & 1 for shift in OP_SEL_HI_SHIFTS]
    neg_lo = [word >> (NEG_LO_SHIFT + i) & 1 for i in range(3)]
    neg_hi = [word >> (NEG_HI_SHIFT + i) & 1 for i in range(3)]

    if name in pkf16.FMA_MIX:
        bindings = ", ".join(
            f"{var} = {source(code, literal)}"
            for var, code, is_f16 in zip("xyz", codes, op_sel_hi)
            if not (is_f16 and code in INLINE_HALVES)
        )
        args = []
        for var, code, is_f16, hi_half, use_abs, use_neg in zip("xyz", codes, op_sel_hi, op_sel, neg_hi, neg_lo):
            expr = f"F16ToF32({_pk_half_expr(var, code, hi_half)})" if is_f16 else var
            if use_abs:
                expr = f"({expr} & 0x7FFFFFFFu)"
            if use_neg:
                expr = f"({expr} ^ 0x80000000u)"
            args.append(expr)
        fma_expr = f"FmaF32({args[0]}, {args[1]}, {args[2]})"
        prefix = f"uint {bindings}; " if bindings else ""
        if name == "v_fma_mix_f32":
            res_expr = f"F32Clamp01({fma_expr})" if clamp else fma_expr
            return [f"{{ {prefix}if ({active}) v[{d}] = {res_expr}; }}"]
        h_expr = f"F32ToF16({fma_expr})"
        if clamp:
            h_expr = f"F16Clamp({h_expr})"
        store = f"(v[{d}] & 0xFFFFu) | (({h_expr}) << 16)" if name == "v_fma_mixhi_f16" else (
            f"(v[{d}] & 0xFFFF0000u) | ({h_expr})")
        return [f"{{ {prefix}if ({active}) v[{d}] = {store}; }}"]

    # v_dot2_f32_f16
    bindings = ", ".join(
        f"{var} = {source(code, literal)}"
        for i, (var, code) in enumerate(zip("xyz", codes))
        if i == 2 or code not in INLINE_HALVES
    )
    a_lo = f"F16ToF32({_pk_half_expr('x', codes[0], op_sel[0], neg_lo[0])})"
    a_hi = f"F16ToF32({_pk_half_expr('x', codes[0], op_sel_hi[0], neg_hi[0])})"
    b_lo = f"F16ToF32({_pk_half_expr('y', codes[1], op_sel[1], neg_lo[1])})"
    b_hi = f"F16ToF32({_pk_half_expr('y', codes[1], op_sel_hi[1], neg_hi[1])})"
    c_expr = "(z ^ 0x80000000u)" if neg_lo[2] else "z"
    dot_expr = f"F32Add(F32Add(F32Mul({a_lo}, {b_lo}), F32Mul({a_hi}, {b_hi})), {c_expr})"
    res_expr = f"F32Clamp01({dot_expr})" if clamp else dot_expr
    prefix = f"uint {bindings}; " if bindings else ""
    return [f"{{ {prefix}if ({active}) v[{d}] = {res_expr}; }}"]


def pk_fmac_vop2(name, fields, literal, active):
    """HLSL lines for VOP2 v_pk_fmac_f16."""
    message = pkf16.pk_fmac_vop2_refusal(name, fields)
    if message:
        raise Untranslatable(f"not translated to HLSL: {message}")
    d = fields["vdst"]
    codes = (fields["src0"], VGPR_FIRST + fields["vsrc1"])
    bindings = _src_bindings(codes, literal)
    a_lo = _pk_half_expr("x", codes[0], False)
    a_hi = _pk_half_expr("x", codes[0], True)
    b_lo = _pk_half_expr("y", codes[1], False)
    b_hi = _pk_half_expr("y", codes[1], True)
    lo = f"F16Fma({a_lo}, {b_lo}, v[{d}] & 0xFFFFu)"
    hi = f"F16Fma({a_hi}, {b_hi}, v[{d}] >> 16)"
    prefix = f"uint {bindings}; " if bindings else ""
    return [f"{{ {prefix}if ({active}) v[{d}] = ({lo}) | (({hi}) << 16); }}"]


def pk_fmac_vop3(name, fields, literal, active):
    """HLSL lines for VOP3 v_pk_fmac_f16."""
    message = pkf16.pk_fmac_vop3_refusal(name, fields)
    if message:
        raise Untranslatable(f"not translated to HLSL: {message}")
    d = fields.vdst
    codes = fields.sources[:2]
    bindings = _src_bindings(codes, literal)
    a_lo = _pk_half_expr("x", codes[0], False)
    a_hi = _pk_half_expr("x", codes[0], True)
    b_lo = _pk_half_expr("y", codes[1], False)
    b_hi = _pk_half_expr("y", codes[1], True)
    lo = f"F16Fma({a_lo}, {b_lo}, v[{d}] & 0xFFFFu)"
    hi = f"F16Fma({a_hi}, {b_hi}, v[{d}] >> 16)"
    if fields.clamp:
        lo, hi = f"F16Clamp({lo})", f"F16Clamp({hi})"
    prefix = f"uint {bindings}; " if bindings else ""
    return [f"{{ {prefix}if ({active}) v[{d}] = ({lo}) | (({hi}) << 16); }}"]
