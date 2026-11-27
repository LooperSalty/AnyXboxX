"""VOP3 and VOP3P integer arithmetic with boundary values in both halves of each operand."""
import pytest

from emu import rdna2_hlsl
from test_rdna2_exec import V, program, vop3
from test_rdna2_exec_int16 import vop3p
from test_rdna2_hlsl_int16 import parity_memory, parity_program
from test_rdna2_hlsl_rust_half import verify
from test_rdna2_hlsl_rust_parity import rust


def test_int16_operations_match_python_text_and_reference_memory():
    data, _ = parity_program()
    verify(data, parity_memory())


@pytest.mark.parametrize("encoder,name,modifier", [
    (vop3, "v_add_nc_u16", {"abs_": 1}),
    (vop3, "v_mad_u32_u16", {"op_sel": 4}),
    (vop3, "v_min_i16", {"clamp": 1}),
    (vop3p, "v_pk_sub_i16", {"neg_lo": 1}),
    (vop3p, "v_pk_lshlrev_b16", {"clamp": 1}),
])
def test_undefined_modifiers_are_named(encoder, name, modifier):
    data = program(*encoder(name, 0, V + 1, V + 2, **modifier))
    with pytest.raises(rdna2_hlsl.Untranslatable, match=name):
        rdna2_hlsl.translate(data)
    result = rust(data)
    assert result.returncode and name.encode() in result.stderr
