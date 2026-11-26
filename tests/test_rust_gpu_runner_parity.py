"""Byte-for-byte parity between the Rust GPU runner and the C++ oracle on WARP."""
import os
import subprocess
from pathlib import Path

import pytest

from emu import rdna2_gpu

ROOT = Path(__file__).resolve().parents[1]
RUST = ROOT / "target" / "release" / "anyxboxx_gpu_run.exe"
ORACLE = ROOT / "runtime" / "build" / "bin" / "anyxboxx_gpu_run_cpp.exe"


@pytest.fixture
def runners():
    oracle = Path(os.environ.get("ANYXBOXX_GPU_ORACLE", str(ORACLE)))
    if not oracle.is_file():
        if os.environ.get("ANYXBOXX_REQUIRE_RUNTIME") == "1":
            pytest.fail(f"C++ GPU oracle is missing: {oracle}")
        pytest.skip("C++ GPU oracle is unavailable")
    if rdna2_gpu.find_dxc() is None:
        pytest.skip("dxc is unavailable")
    assert RUST.is_file(), "build the Rust GPU runner with cargo build --release"
    return oracle, RUST


def invoke(runner, args, gpu="warp"):
    env = {**os.environ, "ANYXBOXX_GPU": gpu}
    return subprocess.run([str(runner), *map(str, args)], capture_output=True, env=env, timeout=120)


def test_adapter_parity(runners):
    cpp, rust = (invoke(runner, ["--adaptateur"]) for runner in runners)
    assert (rust.returncode, rust.stdout, rust.stderr) == (cpp.returncode, cpp.stdout, cpp.stderr)


@pytest.mark.parametrize("groups,initial", [(1, bytes(129)), (3, bytes(range(129)))])
def test_compute_parity(runners, tmp_path, groups, initial):
    shader = rdna2_gpu.compile_hlsl(
        "RWByteAddressBuffer mem : register(u0);\n"
        "[numthreads(8,1,1)] void main(uint3 id : SV_DispatchThreadID) {\n"
        "  mem.Store(id.x * 4, mem.Load(id.x * 4) + id.x + 17);\n} ", tmp_path)
    results = []
    for index, runner in enumerate(runners):
        memory = tmp_path / f"memory{index}.bin"
        memory.write_bytes(initial)
        result = invoke(runner, [shader, memory, groups])
        results.append((result.returncode, result.stdout, result.stderr, memory.read_bytes()))
    assert results[0][0] == 0, results[0][2]
    assert results[1] == results[0]

@pytest.mark.parametrize("pixel", [False, True])
def test_draw_parity(runners, tmp_path, pixel):
    vs = rdna2_gpu.compile_hlsl(
        "RWByteAddressBuffer mem : register(u0);\n"
        "float4 main(uint id : SV_VertexID) : SV_Position {\n"
        "  mem.Store(id * 4, id + 42);\n"
        "  float2 p[3] = {float2(-1,-1), float2(0,1), float2(1,-1)};\n"
        "  return float4(p[id], 0, 1);\n}", tmp_path, "vs_6_0")
    vs_bytes = vs.read_bytes()
    ps = rdna2_gpu.compile_hlsl(
        "float4 main() : SV_Target { return float4(1,0,0,1); }", tmp_path, "ps_6_0")
    vs = tmp_path / "vertex.dxil"
    vs.write_bytes(vs_bytes)
    results = []
    for index, runner in enumerate(runners):
        memory = tmp_path / f"draw{index}.bin"
        memory.write_bytes(bytes(64))
        result = invoke(runner, ["--draw", vs, ps if pixel else "-", memory, 3, 8, 8])
        results.append((result.returncode, result.stdout, result.stderr, memory.read_bytes()))
    assert results[0][0] == 0, results[0][2]
    assert results[1] == results[0]


def test_failure_parity(runners, tmp_path):
    missing = tmp_path / "missing.dxil"
    memory = tmp_path / "memory.bin"
    memory.write_bytes(bytes(16))
    invalid = tmp_path / "invalid.dxil"
    invalid.write_bytes(b"invalid DXIL")
    for args in ([], [missing, memory], [invalid, memory], ["--draw"]):
        results = [invoke(runner, args) for runner in runners]
        assert results[0].returncode != 0
        assert (results[1].returncode, results[1].stdout, results[1].stderr) == (
            results[0].returncode, results[0].stdout, results[0].stderr)
        assert memory.read_bytes() == bytes(16)


def test_missing_named_adapter_parity(runners, tmp_path):
    shader = tmp_path / "shader.dxil"
    shader.write_bytes(b"unused")
    memory = tmp_path / "memory.bin"
    memory.write_bytes(bytes(16))
    for args in (["--adaptateur"], [shader, memory]):
        results = [invoke(runner, args, "anyxboxx-no-such-adapter-918273") for runner in runners]
        assert results[0].returncode != 0
        assert (results[1].returncode, results[1].stdout, results[1].stderr) == (
            results[0].returncode, results[0].stdout, results[0].stderr)
