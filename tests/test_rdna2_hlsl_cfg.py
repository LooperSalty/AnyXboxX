"""Contrôle de flux dans emu/rdna2_hlsl.py :
- un programme d'un seul bloc de base (sans branchement) s'écrit en ligne droite, sans boucle ;
- dès qu'il y a un branchement, une seule boucle `while (pc != END_PC)` et une garde `if (pc == ...)` par
  bloc de base, pour tout graphe (diamants if/else, boucles simples et imbriquées, boucle à deux entrées) ;
  pas de `switch` : WARP plante sur un `switch` dans une boucle qui contient une barrière de groupe ;
- un branchement hors des instructions est refusé par son nom ;
- étages graphiques (`translate_vertex`, `translate_pixel`) : jamais de `break;` ni de `switch`.
"""
import pytest

from emu import rdna2_gpu, rdna2_hlsl, rdna2_hlsl_graphics, rdna2_hlsl_wave64
from test_rdna2_exec import V, const, program, sop1, sop2, sopc, sopp, vop1, vop2
from test_rdna2_graphics import PIXEL_SHADER, VERTEX_SHADER
from test_rdna2_hlsl import compare, store_lane, with_lane_offset

gpu = pytest.mark.skipif(not rdna2_gpu.available(), reason="dxc ou anyxboxx_gpu_run.exe absent")


def test_straight_line_has_no_pc_loop():
    hlsl = rdna2_hlsl.translate(program(vop1("v_mov_b32", 1, const(7))))
    assert "while (" not in hlsl and "pc ==" not in hlsl
    assert "v[1] = 7u;" in hlsl


def test_straight_line_scalar_and_wave64_have_no_pc_loop():
    code = program(sop1("s_mov_b32", 4, const(5)), sop2("s_add_u32", 5, 4, const(1)))
    for hlsl in (rdna2_hlsl.translate(code), rdna2_hlsl_wave64.translate(code)):
        assert "while (pc" not in hlsl and "pc ==" not in hlsl
        assert "s[5] =" in hlsl


def test_self_loop_is_not_straight_line():
    # Un branchement qui revient au début : jamais en ligne droite.
    code = program(sop2("s_sub_u32", 4, 4, const(1)), sopc("s_cmp_lg_u32", 4, const(0)), sopp("s_cbranch_scc1", -3))
    hlsl = rdna2_hlsl.translate(code)
    assert "[loop] while (pc != END_PC)" in hlsl and "if (pc == 0u) {" in hlsl


def test_branch_outside_the_program_is_refused():
    # Fin du programme à 12 : un saut à 24 ne tombe sur aucune instruction.
    with pytest.raises(rdna2_hlsl.Untranslatable, match="cible de branchement invalide : 24"):
        rdna2_hlsl.translate(program(sopp("s_branch", 5), vop1("v_mov_b32", 1, const(7))))


def test_loop_and_if_else_use_one_pc_loop_with_a_guard_per_block():
    # if (s4 == 0) s5 = 10 else s5 = 20; then loop s6 = 3..1
    code = with_lane_offset(
        sop1("s_mov_b32", 4, const(0)),
        sopc("s_cmp_eq_u32", 4, const(0)),
        sopp("s_cbranch_scc0", 2),
        sop1("s_mov_b32", 5, const(10)),
        sopp("s_branch", 1),
        sop1("s_mov_b32", 5, const(20)),
        sop1("s_mov_b32", 6, const(3)),
        sop2("s_add_u32", 5, 5, 6),
        sop2("s_sub_u32", 6, 6, const(1)),
        sopc("s_cmp_lg_u32", 6, const(0)),
        sopp("s_cbranch_scc1", -4),
        vop1("v_mov_b32", 1, 5),
        *store_lane(1, 0),
    )
    hlsl = rdna2_hlsl.translate(code)
    assert "[loop] while (pc != END_PC)" in hlsl and "if (pc == 0u) {" in hlsl
    assert hlsl.count("[loop] while (pc") == 1            # une seule boucle, quel que soit le graphe
    assert "switch (" not in hlsl and "pc = 24u; continue;" in hlsl


def test_nested_loops_share_the_single_pc_loop():
    # Outer loop s4 = 2..1, inner loop s5 = 3..1, s6 += 1
    code = with_lane_offset(
        sop1("s_mov_b32", 6, const(0)),
        sop1("s_mov_b32", 4, const(2)),
        sop1("s_mov_b32", 5, const(3)),
        sop2("s_add_u32", 6, 6, const(1)),
        sop2("s_sub_u32", 5, 5, const(1)),
        sopc("s_cmp_lg_u32", 5, const(0)),
        sopp("s_cbranch_scc1", -4),
        sop2("s_sub_u32", 4, 4, const(1)),
        sopc("s_cmp_lg_u32", 4, const(0)),
        sopp("s_cbranch_scc1", -8),
        vop1("v_mov_b32", 1, 6),
        *store_lane(1, 0),
    )
    hlsl = rdna2_hlsl.translate(code)
    assert hlsl.count("[loop] while (pc") == 1 and "switch (" not in hlsl


def test_irreducible_cfg_uses_the_pc_loop():
    # Two-entry loop: entry -> B2 (or B1 if scc0), B1 -> B2, B2 -> B1 (cycle {B1, B2} with 2 entries).
    code = program(
        sopc("s_cmp_eq_u32", 4, const(0)),
        sopp("s_cbranch_scc0", 1),
        sopp("s_branch", 2),
        # B1:
        sop2("s_add_u32", 5, 5, const(1)),
        # B2:
        sop2("s_sub_u32", 4, 4, const(1)),
        sopc("s_cmp_lg_u32", 4, const(0)),
        sopp("s_cbranch_scc1", -4),
    )
    hlsl = rdna2_hlsl.translate(code)
    assert "[loop] while (pc != END_PC)" in hlsl and "if (pc == 12u) {" in hlsl


def test_graphics_stages_have_no_switch_or_break():
    for hlsl in (rdna2_hlsl_graphics.translate_vertex(VERTEX_SHADER),
                 rdna2_hlsl_graphics.translate_pixel(PIXEL_SHADER, 64)):
        assert "switch (" not in hlsl and "break;" not in hlsl


@gpu
def test_nested_loops_and_branch_match_interpreter():
    code = with_lane_offset(
        sop1("s_mov_b32", 6, const(0)),
        sop1("s_mov_b32", 4, const(2)),
        sop1("s_mov_b32", 5, const(3)),
        sop2("s_add_u32", 6, 6, const(1)),
        sop2("s_sub_u32", 5, 5, const(1)),
        sopc("s_cmp_lg_u32", 5, const(0)),
        sopp("s_cbranch_scc1", -4),
        sop2("s_sub_u32", 4, 4, const(1)),
        sopc("s_cmp_lg_u32", 4, const(0)),
        sopp("s_cbranch_scc1", -8),
        vop2("v_add_nc_u32", 1, 6, V + 0),
        *store_lane(1, 0),
    )
    expected, actual = compare(code)
    assert actual == expected


@gpu
def test_if_else_diamond_matches_interpreter():
    # if (s4 == 0) s5 = 10 else s5 = 20; v1 = s5 + lane
    code = with_lane_offset(
        sopc("s_cmp_eq_u32", 4, const(0)),
        sopp("s_cbranch_scc0", 2),
        sop1("s_mov_b32", 5, const(10)),
        sopp("s_branch", 1),
        sop1("s_mov_b32", 5, const(20)),
        vop2("v_add_nc_u32", 1, 5, V + 0),
        *store_lane(1, 0),
    )
    for branch_value in (0, 1):
        expected, actual = compare(code, sgprs={4: branch_value})
        assert actual == expected


@gpu
def test_irreducible_cfg_matches_interpreter():
    # Two-entry loop (irreducible CFG):
    # entry -> B2 when s4 == 0, else B1; B1 adds 1 to s5 then falls into B2;
    # B2 decrements s4 and loops back to B1 while s4 != 0.
    code = with_lane_offset(
        sopc("s_cmp_eq_u32", 4, const(0)),
        sopp("s_cbranch_scc1", 2),
        sopp("s_branch", 0),
        # B1:
        sop2("s_add_u32", 5, 5, const(1)),
        # B2:
        sopc("s_cmp_eq_u32", 4, const(0)),
        sopp("s_cbranch_scc1", 3),
        sop2("s_sub_u32", 4, 4, const(1)),
        sopc("s_cmp_lg_u32", 4, const(0)),
        sopp("s_cbranch_scc1", -6),
        vop2("v_add_nc_u32", 1, 5, V + 0),
        *store_lane(1, 0),
    )
    assert "[loop] while (pc != END_PC)" in rdna2_hlsl.translate(code)
    for start_s4 in (0, 3):
        expected, actual = compare(code, sgprs={4: start_s4, 5: 10})
        assert actual == expected
