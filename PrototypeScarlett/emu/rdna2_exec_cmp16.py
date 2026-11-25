"""16-bit integer compares of the RDNA 2 reference interpreter: v_cmp[x]_{lt,eq,le,gt,ne,ge}_{i16,u16}.

They compare S0[15:0] with S1[15:0], signed (i16) or unsigned (u16); the high halves are ignored. Like the
32-bit compares, the mask is written whole: bits of inactive lanes are cleared. VOPC writes VCC (v_cmp) or
EXEC (v_cmpx), VOP3 an SGPR (v_cmp) or EXEC (v_cmpx). There are no f/t forms for these types on RDNA 2.

Sources:
- AMD, "RDNA 2" Instruction Set Architecture, November 2020, table 61 "VOPC Compare Opcodes"
  (p. 156-159: 137 V_CMP_LT_I16 to 190 V_CMPX_GE_U16, "D[threadId] = (S0 < S1)"), section 3.9 "Vector
  Compares: VCC and VCCZ" (p. 19-20, "VCC is fully written"). Table 60's hex ranges (0xA0-0xBF) do not
  match table 61 or LLVM; the opcodes come from table 61 and LLVM (rdna2_ops.py, 0x89-0xBE).
- Same document, section 6.2 "Operands" (p. 41-42): codes 240-248 are "Single, double, or half-precision
  inline floats"; a 16-bit operand reads the half-precision value (1/(2*PI) = 0x3118). LLVM 20.1.8 decodes
  them the same way for 16-bit integer operands: VSrc_b16/SSrc_b16 have an immediate width of 16
  (SIRegisterInfo.td), and AMDGPUDisassembler::decodeFPImmed maps width 16 to getInlineImmValF16
  (240 → 0x3800 ... 248 → 0x3118). VOPC SDWA forms with an inline float source are not modelled and are
  refused.
- Same document, section 6.2.5 "OPSEL Field Restrictions" (p. 44): OPSEL is "not for VOP1, VOP2 or VOPC
  instructions using the VOP3 encoding (these should use SDWA instead)": a VOP3 compare with op_sel set
  is refused by name instead of guessing which half the hardware reads.
"""
HALF_MASK, HALF_SIGN = 0xFFFF, 0x8000
# Half-precision bits of the inline float constants (ISA section 6.2).
HALF_INLINE_FLOATS = {240: 0x3800, 241: 0xB800, 242: 0x3C00, 243: 0xBC00, 244: 0x4000, 245: 0xC000,
                      246: 0x4400, 247: 0xC400, 248: 0x3118}
TESTS = {"lt": lambda x, y: x < y, "eq": lambda x, y: x == y, "le": lambda x, y: x <= y,
         "gt": lambda x, y: x > y, "ne": lambda x, y: x != y, "ge": lambda x, y: x >= y}


def unsigned16(value):
    return value & HALF_MASK


def signed16(value):
    value &= HALF_MASK
    return value - (HALF_MASK + 1) if value & HALF_SIGN else value


# {mnemonic: (test on the two 32-bit source values, writes EXEC instead of VCC/SDST)}
COMPARE16 = {}
for _prefix, _writes_exec in (("v_cmp", False), ("v_cmpx", True)):
    for _name, _test in TESTS.items():
        COMPARE16[f"{_prefix}_{_name}_i16"] = (lambda a, b, t=_test: t(signed16(a), signed16(b)), _writes_exec)
        COMPARE16[f"{_prefix}_{_name}_u16"] = (lambda a, b, t=_test: t(unsigned16(a), unsigned16(b)), _writes_exec)


def operand(name, code, value):
    """Value of source `code` (already read as 32 bits) for compare `name`: half inline floats for 16 bits."""
    return HALF_INLINE_FLOATS[code] if name in COMPARE16 and code in HALF_INLINE_FLOATS else value


def check_vop3(name, op_sel, error):
    """Refuse a VOP3 16-bit compare with op_sel set (ISA section 6.2.5); `error`: exception class to raise."""
    if name in COMPARE16 and op_sel:
        raise error(f"{name} with op_sel (VOP3 compare, 16-bit sources): not usable per the RDNA 2 ISA")


def check_sdwa_inline(name, codes, error):
    """Refuse a 16-bit SDWA compare whose scalar source is an inline float (half or single not settled)."""
    if name in COMPARE16 and any(code in HALF_INLINE_FLOATS for code in codes):
        raise error(f"{name} in SDWA with an inline float source: 16-bit value not modelled")
