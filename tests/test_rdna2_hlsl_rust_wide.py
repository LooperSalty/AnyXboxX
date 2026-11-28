"""Wide VGPR pairs, f64 ALU, shifts and bit-exact f32 widening on WARP."""
from test_rdna2_exec_wide import f32_sources_memory, f32_to_double_program, wide_program
from test_rdna2_exec_div import div_memory, div_program
from test_rdna2_hlsl_rust_half import verify


def test_wide_operations():
    verify(wide_program(), b"")


def test_widen_f32_preserves_nan_payloads_and_subnormals():
    verify(f32_to_double_program(), f32_sources_memory())


def test_division_instructions_and_vcc():
    verify(div_program(), div_memory())
