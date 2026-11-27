"""Every typed buffer encoding, including packed d16 and high-half loads/stores."""
import pytest

from emu import rdna2_exec, rdna2_gpu, rdna2_hlsl
from test_rdna2_exec import LITERAL, sop1
from test_rdna2_exec_format import OPCODES, XYZW, typed
from test_rdna2_hlsl import store_lane, with_lane_offset
from test_rdna2_hlsl_format import element_bytes, store_values
from test_rdna2_hlsl_rust_parity import rust


@pytest.mark.parametrize("name", sorted(name for family, name in OPCODES
                                         if family in ("MTBUF", "MUBUF") and "_format_" in name))
def test_formatted_access(name):
    count = len(name.rsplit("_", 1)[1])
    fmt = (5, 27, 34, 60)[count - 1]
    selects = 0o1154 if count == 2 else XYZW
    memory = element_bytes(79) + bytes([0xa5]) * 512
    words = list(store_values())
    for sgpr, value in [(2, 512), (21, 16 << 16), (22, 25), (23, fmt << 12 | selects), (126, 0x55555555)]:
        words.extend([sop1("s_mov_b32", sgpr, LITERAL), value])
    words += typed(name, idxen=1, fmt=fmt)
    words += [sop1("s_mov_b32", 126, LITERAL), 0xffffffff]
    for index in range(4):
        words += store_lane(8 + index, index)
    data = with_lane_offset(*words)
    result = rust(data)
    assert result.returncode == 0, result.stderr.decode()
    hlsl = result.stdout.decode()
    assert hlsl == rdna2_hlsl.translate(data)
    if not rdna2_gpu.available():
        pytest.skip("dxc or GPU runner is unavailable")
    wave = rdna2_exec.Wave()
    wave.memory.write(0, memory)
    rdna2_exec.run(data, wave)
    assert rdna2_gpu.run(hlsl, memory) == wave.memory.read(0, len(memory))
