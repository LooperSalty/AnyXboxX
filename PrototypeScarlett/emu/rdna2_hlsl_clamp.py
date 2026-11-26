"""HLSL of the 32-bit integer add/subtract with carry-out, and of VOP3 clamp on 32-bit integer results.

Semantics and sources: emu/rdna2_exec_vop3 (CARRY, SATURATED_U32). With clamp, the saturated value is chosen from
the carry or borrow with integer compares, never by clamp() on a widened sum. Clamp on any other integer result is
refused by name, as in the interpreter. Shared by wave32 and wave64.
"""
from emu.rdna2_exec_vop3 import CARRY_SATURATION, INTEGER_CLAMP, SATURATED_U32
from emu.rdna2_hlsl_ops import Untranslatable

# (D, carry or borrow) of each carry op, from x = S0, y = S1, k = carry-in bit, r = D.
CARRY = {
    "v_add_co_u32": ("x + y", "r < x"),
    "v_sub_co_u32": ("x - y", "x < y"),
    "v_subrev_co_u32": ("y - x", "y < x"),
    "v_add_co_ci_u32": ("x + y + k", "(r < x) || (k != 0 && r == x)"),
    "v_sub_co_ci_u32": ("x - y - k", "(x < y) || (k != 0 && x == y)"),
    "v_subrev_co_ci_u32": ("y - x - k", "(y < x) || (k != 0 && y == x)"),
}
SATURATED_U32_HLSL = {
    "v_add_nc_u32": "({a} + {b} < {a} ? 0xFFFFFFFFu : {a} + {b})",
    "v_sub_nc_u32": "({a} < {b} ? 0u : {a} - {b})",
    "v_subrev_nc_u32": "({b} < {a} ? 0u : {b} - {a})",
}


def refuse_integer_clamp(name, clamp):
    """Untranslatable for clamp on an integer result that is not implemented (emu/rdna2_exec_vop3.INTEGER_CLAMP)."""
    if clamp:
        raise Untranslatable(f"non traduite en HLSL : {name} {INTEGER_CLAMP}")


def integer_template(name, clamp, template):
    """Expression template of a 32-bit integer VOP3 op: saturated with clamp on the _nc_u32 ops, else refused."""
    if not clamp:
        return template
    if name not in SATURATED_U32:
        refuse_integer_clamp(name, clamp)
    return SATURATED_U32_HLSL[name]


def carry_write(name, clamp):
    """Value written to D by a carry op: r, or with VOP3 clamp the saturated value on a carry/borrow."""
    return f"(o ? 0x{CARRY_SATURATION[name]:X}u : r)" if clamp else "r"
