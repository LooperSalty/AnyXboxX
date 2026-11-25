"""f32 minimum, maximum and median of the RDNA 2 reference interpreter (v_min/max_f32, v_min3/max3/med3_f32).

AMD, "RDNA 2" Instruction Set Architecture (November 2020), V_MIN_F32 and V_MAX_F32 (section 12.7, VOP2
opcodes 15 and 16, p. 132-133): a NaN operand gives the other operand ("else if (S0.f == NaN) D.f = S1.f;
else if (S1.f == NaN) D.f = S0.f"), and -0.0 counts as less than +0.0 ("S0.f == +0.0 && S1.f == -0.0" gives
S1.f for min, S0.f for max). V_MIN3_F32 and V_MAX3_F32 chain them (section 12.12, VOP3 opcodes 337 and 340,
p. 170); V_MED3_F32 (opcode 343) returns V_MIN3_F32 of the three sources when one of them is NaN.

Python's min and max do neither: min(nan, 1.0) is nan and max(-0.0, 0.0) is -0.0. The IEEE_MODE cases
(a signalling NaN comes back quieted) are not modelled, as for v_min/max_f16 and v_min/max_f64: every NaN
is non-signalling once read (rdna2_exec_ops.bits_float).
"""
import math


def _negative(x):
    return math.copysign(1.0, x) < 0


def minimum(x, y):
    """V_MIN_F32 of two floats."""
    if math.isnan(x):
        return y
    if math.isnan(y):
        return x
    if x == y == 0:
        return x if _negative(x) else y
    return x if x < y else y


def maximum(x, y):
    """V_MAX_F32 of two floats."""
    if math.isnan(x):
        return y
    if math.isnan(y):
        return x
    if x == y == 0:
        return y if _negative(x) else x
    return x if x > y else y


def minimum3(x, y, z):
    return minimum(minimum(x, y), z)


def maximum3(x, y, z):
    return maximum(maximum(x, y), z)


def median3(x, y, z):
    """V_MED3_F32: the maximum of the two sources left once the largest is taken out."""
    if math.isnan(x) or math.isnan(y) or math.isnan(z):
        return minimum3(x, y, z)
    largest = maximum3(x, y, z)
    if largest == x:
        return maximum(y, z)
    if largest == y:
        return maximum(x, z)
    return maximum(x, y)
