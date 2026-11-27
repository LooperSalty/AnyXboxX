"""All supported MIMG mnemonics with real image and sampler descriptors."""
import pytest

from emu import rdna2_exec, rdna2_exec_gather as gather, rdna2_gpu, rdna2_hlsl, rdna2_image as images
from test_rdna2_exec import LITERAL, const, sop1, vop1
from test_rdna2_hlsl import store_lane, with_lane_offset
from test_rdna2_hlsl_format import element_bytes
from test_rdna2_hlsl_rust_parity import rust
from test_rdna2_image import descriptors, mimg
from test_rdna2_image_atomic import mimg_atomic


@pytest.mark.parametrize("name", sorted(images.SUPPORTED_OPS | gather.OPERATIONS))
def test_image_text_and_memory(name):
    atomic = name in images.ATOMIC_OPS
    sampling = name in images.SAMPLER_OPS or name in gather.OPERATIONS
    fmt = images.FORMAT_32_FLOAT if atomic else images.FORMAT_8_8_8_8_UNORM
    state = descriptors(fmt, 4, 4, images.CLAMP_WRAP, address=512)
    words = []
    for reg, value in state.items():
        words.extend((sop1("s_mov_b32", reg, LITERAL), value))
    # Distinct texels and destination seeds make partial masks and store results observable.
    for reg in range(4, 10):
        words.extend((vop1("v_mov_b32", reg, LITERAL), 0x3f000000 if sampling else int(reg == 4)))
    for reg in range(12, 16):
        words.extend((vop1("v_mov_b32", reg, LITERAL), 0x3f400000 + reg))
    words.append(sop1("s_mov_b32", 126, const(1)))
    if atomic:
        words.extend(mimg_atomic(name, 12, 4, dmask=3 if name in images.ATOMIC_CMPSWAP_OPS else 1))
    else:
        mask = 1 if "gather" in name else 0b1011
        # The extended gather with offset, bias, compare and clamp has six addresses.
        nsa = None if name in gather.OPERATIONS and gather.layout(name)["count"] > 5 else [4, 6, 5, 8, 7]
        words.extend(mimg(name, 12, 4, dmask=mask, nsa_addrs=nsa))
    words.extend((sop1("s_mov_b32", 126, LITERAL), 0xffffffff))
    for i in range(4):
        words.extend(store_lane(12 + i, i))
    data = with_lane_offset(*words)
    result = rust(data)
    assert result.returncode == 0, result.stderr.decode()
    hlsl = result.stdout.decode()
    assert hlsl == rdna2_hlsl.translate(data)
    if not rdna2_gpu.available():
        pytest.skip("dxc or GPU runner is unavailable")
    start = bytes(512) + element_bytes(31)
    wave = rdna2_exec.Wave()
    wave.memory.write(0, start)
    rdna2_exec.run(data, wave)
    assert rdna2_gpu.run(hlsl, start) == wave.memory.read(0, len(start))
