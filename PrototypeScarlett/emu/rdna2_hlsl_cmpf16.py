"""Half-precision compares v_cmp[x]_*_f16 and v_cmp[x]_class_f16 in the HLSL recompiler (wave32 and wave64).

Same semantics as emu/rdna2_exec_cmpf16.py. Sources are read like the other f16 instructions
(rdna2_hlsl_f16._operand: a register's low half, an inline float as binary16, an integer or a literal as its low
16 bits; VOP3 abs/neg on bit 15). The 16 predicates widen both halves with F16ToF32, which is exact (an f16
denormal is a normal f32), and reuse the f32 conditions of rdna2_hlsl_ops, NaN tested apart. The class test
reads the f16 bits directly (ClassF16): widening would turn a signalling NaN quiet and a denormal normal.
Forms the interpreter refuses (op_sel, clamp, omod, abs/neg on the class mask, SDWA/DPP) are refused here too.
"""
from emu.rdna2_exec_cmpf16 import COMPAREF16, MASK
from emu.rdna2_exec_f16 import EXTENSIONS, VGPR_FIRST
from emu.rdna2_hlsl_f16 import _modified, _operand
from emu.rdna2_hlsl_ops import Untranslatable, compare_condition

HALF = "h"
HELPERS = """uint ClassF16(uint h) {
  uint e = (h >> 10) & 0x1Fu, m = h & 0x3FFu, n = (h >> 15) & 1u;
  if (e == 0x1Fu) return m != 0u ? ((m >> 9) & 1u) : (n != 0u ? 2u : 9u);
  if (e == 0u) return m == 0u ? (n != 0u ? 5u : 6u) : (n != 0u ? 4u : 7u);
  return n != 0u ? 3u : 8u;
}"""


def condition(name, a, b):
    """Boolean HLSL expression of the compare on the binary16 values `a` and `b` (uint, low 16 bits)."""
    if name.endswith("_class_f16"):
        return f"((({b}) >> ClassF16({a})) & 1u) != 0u"
    return compare_condition(name[:-len("_f16")] + "_f32", f"F16ToF32({a})", f"F16ToF32({b})")


def vopc_sources(name, fields, literal):
    """(S0, S1) of a VOPC form: src0 and the VGPR vsrc1."""
    if fields["src0"] in EXTENSIONS:
        raise Untranslatable(f"{name}: SDWA/DPP form (src0 = 0x{fields['src0']:X}) not supported for f16")
    return (_operand(name, HALF, fields["src0"], literal),
            _operand(name, HALF, VGPR_FIRST + fields["vsrc1"], literal))


def vop3_sources(name, fields, literal):
    """(S0, S1) of a VOP3 form, abs/neg applied on bit 15; unmeasured forms refused."""
    kinds = COMPAREF16[name][2]
    if fields.op_sel:
        raise Untranslatable(f"{name} with op_sel (VOP3 compare, 16-bit sources): not usable per the RDNA 2 ISA")
    if fields.clamp or fields.omod:
        raise Untranslatable(f"{name} with clamp or omod (VOP3 f16 compare): not measured on RDNA 2")
    if any(kind == MASK and (fields.abs | fields.neg) >> index & 1 for index, kind in enumerate(kinds)):
        raise Untranslatable(f"{name} with abs/neg on the class mask S1: not measured on RDNA 2")
    return tuple(_modified(_operand(name, HALF, code, literal), kind, fields, index)
                 for index, (kind, code) in enumerate(zip(kinds, fields.sources[:2])))
