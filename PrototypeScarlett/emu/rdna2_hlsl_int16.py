"""HLSL translation of the 16-bit integer VALU (VOP3 and packed VOP3P), semantics of emu/rdna2_exec_int16.py.

Everything is computed in 32-bit `uint`/`int`: the 16-bit halves are zero-extended (`Sext16` sign-extends them
for the i16 ops), no intermediate value overflows 32 bits (|i16 x i16 + i16| < 2^31), and the result is masked
to 16 bits when it is stored in its half. The 32-bit saturations detect overflow from the operand and result
signs instead of using 64-bit integers (Intel UHD and WARP run it the same way). The forms the interpreter
refuses are refused here with the same message. The text reads neither EXEC bit by bit nor another lane: it
serves as is in wave64 (condition `LANE_ON`).
"""
from emu import rdna2_exec_int16 as int16
from emu.rdna2_hlsl_ops import Untranslatable, source
from emu.rdna2_operands_vop3p import (CLAMP_BIT, OP_SEL_HI_SHIFTS, OP_SEL_SHIFT, SOURCE_MASK, SOURCE_SHIFTS,
                                      VDST_MASK)

TRANSLATED = int16.VOP3 | frozenset(int16.PACKED)

HELPERS = """int Sext16(uint x) {
  // Low 16 bits of x, sign-extended (bits 31:16 are ignored).
  return asint(x << 16) >> 16;
}
uint SatI16(int x) { return asuint(clamp(x, -32768, 32767)); }
uint AddSatI16(uint a, uint b, bool subtract) {
  // Signed 16-bit a + b (or a - b) saturated, from the sign bits in uint. Written as
  // SatI16(Sext16(a) + Sext16(b)) then masked to 16 bits, the sign-extended high half reached the
  // destination on an NVIDIA RTX 4060 Laptop GPU (WARP and Intel UHD were right).
  uint r = (subtract ? a - b : a + b) & 0xFFFFu;
  uint o = subtract ? (a ^ b) & (a ^ r) : (a ^ r) & (b ^ r);
  return (o & 0x8000u) != 0u ? ((a & 0x8000u) != 0u ? 0x8000u : 0x7FFFu) : r;
}
uint AddSatI32(uint a, uint b, bool sat) {
  // a + b, or with `sat` the signed sum saturated to [0x80000000, 0x7FFFFFFF].
  uint r = a + b;
  bool overflow = ((a ^ r) & (b ^ r) & 0x80000000u) != 0u;
  return sat && overflow ? ((a & 0x80000000u) != 0u ? 0x80000000u : 0x7FFFFFFFu) : r;
}
uint SubSatI32(uint a, uint b, bool sat) {
  uint r = a - b;
  bool overflow = ((a ^ b) & (a ^ r) & 0x80000000u) != 0u;
  return sat && overflow ? ((a & 0x80000000u) != 0u ? 0x80000000u : 0x7FFFFFFFu) : r;
}
"""
# Half operation: (expression without clamp, expression with clamp or None); {a} {b} {c}: zero-extended halves.
TEMPLATES = {
    "add_u16": ("({a} + {b})", "min({a} + {b}, 0xFFFFu)"),
    "sub_u16": ("({a} - {b})", "({a} > {b} ? {a} - {b} : 0u)"),
    "add_i16": ("({a} + {b})", "AddSatI16({a}, {b}, false)"),
    "sub_i16": ("({a} - {b})", "AddSatI16({a}, {b}, true)"),
    "mul_lo_u16": ("({a} * {b})", None),
    "lshlrev_b16": ("({b} << ({a} & 15u))", None),
    "lshrrev_b16": ("({b} >> ({a} & 15u))", None),
    "ashrrev_i16": ("asuint(Sext16({b}) >> int({a} & 15u))", None),
    "max_u16": ("max({a}, {b})", None),
    "max_i16": ("asuint(max(Sext16({a}), Sext16({b})))", None),
    "min_u16": ("min({a}, {b})", None),
    "min_i16": ("asuint(min(Sext16({a}), Sext16({b})))", None),
    "mad_u16": ("({a} * {b} + {c})", "min({a} * {b} + {c}, 0xFFFFu)"),
    "mad_i16": ("asuint(Sext16({a}) * Sext16({b}) + Sext16({c}))", "SatI16(Sext16({a}) * Sext16({b}) + Sext16({c}))"),
    "min3_u16": ("min(min({a}, {b}), {c})", None),
    "min3_i16": ("asuint(min(min(Sext16({a}), Sext16({b})), Sext16({c})))", None),
    "max3_u16": ("max(max({a}, {b}), {c})", None),
    "max3_i16": ("asuint(max(max(Sext16({a}), Sext16({b})), Sext16({c})))", None),
    "med3_u16": ("max(min({a}, {b}), min(max({a}, {b}), {c}))", None),
    "med3_i16": ("asuint(max(min(Sext16({a}), Sext16({b})), min(max(Sext16({a}), Sext16({b})), Sext16({c}))))",
                 None),
}
PACK_32 = {
    "v_cvt_pk_u16_u32": "min(x, 0xFFFFu) | (min(y, 0xFFFFu) << 16)",
    "v_cvt_pk_i16_i32": "(SatI16(asint(x)) & 0xFFFFu) | (SatI16(asint(y)) << 16)",
}


def _half(name, high):
    return f"({name} >> 16)" if high else f"({name} & 0xFFFFu)"


def _expression(operation, selects, clamp):
    """HLSL expression whose low 16 bits are the result; `selects`: high-half bit of each source x, y, z."""
    plain, saturated = TEMPLATES[operation]
    halves = {key: _half(variable, high) for key, variable, high in zip("abc", "xyz", selects)}
    return (saturated if clamp else plain).format(**halves)


def _sources(codes, literal):
    return ", ".join(f"{variable} = {source(code, literal)}" for variable, code in zip("xyz", codes))


def vop3(name, fields, literal, active):
    """HLSL lines of a VOP3 instruction of rdna2_exec_int16.VOP3; `fields`: emu.rdna2_exec_vop3.Fields."""
    message = int16.vop3_refusal(name, fields)
    if message:
        raise Untranslatable(f"not translated to HLSL: {message}")
    d, op_sel, clamp = fields.vdst, fields.op_sel, bool(fields.clamp)
    if name in int16.VOP3_HALF:
        operation = int16.VOP3_HALF[name][0]
        count = int16.HALF_OPERATIONS[operation][1]
        result = _expression(operation, [op_sel >> i & 1 for i in range(count)], clamp)
        if op_sel & int16.DESTINATION_HIGH:
            store = f"(v[{d}] & 0xFFFFu) | (r << 16)"
        else:
            store = f"(v[{d}] & 0xFFFF0000u) | (r & 0xFFFFu)"
        return [f"{{ uint {_sources(fields.sources[:count], literal)}; uint r = {result}; "
                f"if ({active}) v[{d}] = {store}; }}"]
    if name in int16.MAD_32_16:
        return [_mad_32_16(name, fields, literal, active)]
    if name in int16.PACK_32:
        return [f"{{ uint {_sources(fields.sources[:2], literal)}; if ({active}) v[{d}] = {PACK_32[name]}; }}"]
    function = "AddSatI32" if name == "v_add_nc_i32" else "SubSatI32"
    sat = "true" if clamp else "false"
    return [f"{{ uint {_sources(fields.sources[:2], literal)}; if ({active}) v[{d}] = {function}(x, y, {sat}); }}"]


def _mad_32_16(name, fields, literal, active):
    """S0.half x S1.half + S2; clamp saturates the 32-bit sum (op_sel[2] and op_sel[3] are refused before)."""
    op_sel = fields.op_sel
    a, b = _half("x", op_sel & 1), _half("y", op_sel >> 1 & 1)
    if int16.MAD_32_16[name]:
        product = f"asuint(Sext16({a}) * Sext16({b}))"
        total = f"AddSatI32(p, z, {'true' if fields.clamp else 'false'})"
    else:
        product = f"{a} * {b}"
        total = "(p + z < p ? 0xFFFFFFFFu : p + z)" if fields.clamp else "p + z"
    return (f"{{ uint {_sources(fields.sources, literal)}; uint p = {product}; uint r = {total}; "
            f"if ({active}) v[{fields.vdst}] = r; }}")


def packed(name, word, literal, active):
    """HLSL lines of a packed VOP3P instruction of rdna2_exec_int16.PACKED; `word`: the two instruction words."""
    if name not in int16.PACKED:
        raise Untranslatable(f"not translated to HLSL: {name} (VOP3P)")
    message = int16.packed_refusal(name, word)
    if message:
        raise Untranslatable(f"not translated to HLSL: {message}")
    operation, clamp_effect = int16.PACKED[name]
    count = int16.HALF_OPERATIONS[operation][1]
    clamp = bool(word >> CLAMP_BIT & 1) and clamp_effect == int16.SATURATE
    codes = [word >> shift & SOURCE_MASK for shift in SOURCE_SHIFTS[:count]]
    low = _expression(operation, [word >> (OP_SEL_SHIFT + i) & 1 for i in range(count)], clamp)
    high = _expression(operation, [word >> shift & 1 for shift in OP_SEL_HI_SHIFTS[:count]], clamp)
    return [f"{{ uint {_sources(codes, literal)}; uint lo = {low}, hi = {high}; "
            f"if ({active}) v[{word & VDST_MASK}] = (lo & 0xFFFFu) | (hi << 16); }}"]


__all__ = ["HELPERS", "TRANSLATED", "packed", "vop3"]
