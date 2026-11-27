"""Every packed binary16, mixed FMA and dot operation against the WARP memory oracle."""
from test_rdna2_hlsl_pkf16 import packed_program
from test_rdna2_hlsl_rust_half import verify


def test_packed_half_text_and_warp():
    verify(packed_program(), b"")
