"""NaN and signed-zero rules of v_min/max_f32 and v_min3/max3/med3_f32 in the RDNA 2 interpreter.

Expected values follow the pseudo-code of AMD's "RDNA 2" ISA (November 2020): V_MIN_F32 and V_MAX_F32
(p. 132-133) give the other operand when one is NaN and order -0.0 below +0.0; V_MIN3_F32, V_MAX3_F32 and
V_MED3_F32 (p. 170) are built on them. Before the fix, Python's min and max returned a NaN in S0 and kept
S0 when the two zeros differed in sign. The V_MED3_F32 NaN cases also match LLVM's constant folding of
llvm.amdgcn.fmed3 (opt -passes=instcombine, gfx1030).
"""
from emu import rdna2_exec
from emu.rdna2_exec_ops import float_bits
from test_rdna2_exec import V, program, vop1, vop2, vop3

LITERAL, NEG_SRC0 = 255, 1
NAN, NEG_NAN, PLUS_ZERO, NEG_ZERO = 0x7FC0_0000, 0xFFC0_0000, 0, 0x8000_0000
PLUS_INF, MINUS_INF = 0x7F80_0000, 0xFF80_0000
ONE, TWO, MINUS_ONE = float_bits(1.0), float_bits(2.0), float_bits(-1.0)


def run(sources, *words):
    """Sources in v1, v2, v3; result read in v4 of lane 0."""
    moves = []
    for index, value in enumerate(sources):
        moves += [vop1("v_mov_b32", 1 + index, LITERAL), value]
    return rdna2_exec.run(program(*moves, *words)).v[4][0]


TWO_SOURCES = [
    # name, S0, S1, expected
    ("v_min_f32", NAN, ONE, ONE), ("v_min_f32", ONE, NAN, ONE),
    ("v_max_f32", NAN, ONE, ONE), ("v_max_f32", ONE, NAN, ONE),
    ("v_min_f32", NEG_NAN, MINUS_INF, MINUS_INF), ("v_max_f32", PLUS_INF, NEG_NAN, PLUS_INF),
    ("v_min_f32", NAN, NEG_NAN, NEG_NAN), ("v_max_f32", NEG_NAN, NAN, NAN),       # both NaN: S1
    ("v_min_f32", PLUS_ZERO, NEG_ZERO, NEG_ZERO), ("v_min_f32", NEG_ZERO, PLUS_ZERO, NEG_ZERO),
    ("v_max_f32", PLUS_ZERO, NEG_ZERO, PLUS_ZERO), ("v_max_f32", NEG_ZERO, PLUS_ZERO, PLUS_ZERO),
    ("v_min_f32", TWO, MINUS_ONE, MINUS_ONE), ("v_max_f32", TWO, MINUS_ONE, TWO),
]


def test_min_max_f32_in_vop2_and_vop3():
    results = {(name, f"{a:08X}", f"{b:08X}"): (f"{run((a, b), vop2(name, 4, V + 1, 2)):08X}",
                                                f"{run((a, b), *vop3(name, 4, V + 1, V + 2)):08X}")
               for name, a, b, _ in TWO_SOURCES}
    assert results == {(name, f"{a:08X}", f"{b:08X}"): (f"{expected:08X}",) * 2 for name, a, b, expected in TWO_SOURCES}


def test_vop3_negated_zero_is_below_plus_zero():
    """v_max_f32 v4, -v1, v1 with v1 = +0: max(-0, +0) = +0, and min gives -0."""
    assert run((PLUS_ZERO,), *vop3("v_max_f32", 4, V + 1, V + 1, neg=NEG_SRC0)) == PLUS_ZERO
    assert run((PLUS_ZERO,), *vop3("v_min_f32", 4, V + 1, V + 1, neg=NEG_SRC0)) == NEG_ZERO


THREE_SOURCES = [
    # name, S0, S1, S2, expected
    ("v_min3_f32", NAN, ONE, PLUS_ZERO, PLUS_ZERO), ("v_min3_f32", TWO, ONE, NAN, ONE),
    ("v_max3_f32", NAN, ONE, PLUS_ZERO, ONE), ("v_max3_f32", ONE, NAN, TWO, TWO),
    ("v_min3_f32", PLUS_ZERO, NEG_ZERO, PLUS_ZERO, NEG_ZERO), ("v_max3_f32", NEG_ZERO, PLUS_ZERO, NEG_ZERO, PLUS_ZERO),
    # V_MED3_F32 with a NaN source: V_MIN3_F32 of the three
    ("v_med3_f32", NAN, ONE, PLUS_ZERO, PLUS_ZERO), ("v_med3_f32", ONE, NAN, PLUS_ZERO, PLUS_ZERO),
    ("v_med3_f32", PLUS_ZERO, ONE, NAN, PLUS_ZERO), ("v_med3_f32", NAN, TWO, ONE, ONE),
    ("v_med3_f32", ONE, TWO, NAN, ONE), ("v_med3_f32", NAN, NAN, TWO, TWO),
    ("v_med3_f32", NAN, PLUS_ZERO, ONE, PLUS_ZERO),          # med3(x, 0, 1.0): the clamp compilers emit
    # without NaN: the median
    ("v_med3_f32", TWO, MINUS_ONE, ONE, ONE), ("v_med3_f32", MINUS_ONE, ONE, TWO, ONE),
    ("v_med3_f32", ONE, MINUS_ONE, TWO, ONE),
]


def test_min3_max3_med3_f32():
    def key(name, a, b, c):
        return name, f"{a:08X}", f"{b:08X}", f"{c:08X}"
    results = {key(name, a, b, c): f"{run((a, b, c), *vop3(name, 4, V + 1, V + 2, V + 3)):08X}"
               for name, a, b, c, _ in THREE_SOURCES}
    assert results == {key(name, a, b, c): f"{expected:08X}" for name, a, b, c, expected in THREE_SOURCES}

