"""Draw with Rust-generated VS and PS text and check the interpreter's export records."""
import struct

import pytest

import test_rdna2_graphics as scene
import test_rdna2_graphics_wave as wave
from emu import rdna2_hlsl_graphics as oracle
from test_rdna2_exec import LITERAL, sop1
from test_rdna2_hlsl_rust_parity import rust


@pytest.fixture
def graphics_backend(monkeypatch):
    vertex, pixel = oracle.translate_vertex, oracle.translate_pixel

    def initialized(data, sgprs):
        prefix = [word for reg, value in sorted((sgprs or {}).items()) for word in (sop1("s_mov_b32", reg, LITERAL), value)]
        return struct.pack(f"<{len(prefix)}I", *prefix) + data

    def translate_vertex(data, sgprs=None):
        data = initialized(data, sgprs)
        result = rust(data, "--vertex")
        assert result.returncode == 0, result.stderr.decode()
        assert result.stdout.decode() == vertex(data)
        return result.stdout.decode()

    def translate_pixel(data, width, sgprs=None):
        data = initialized(data, sgprs)
        result = rust(data, "--pixel", str(width))
        assert result.returncode == 0, result.stderr.decode()
        assert result.stdout.decode() == pixel(data, width)
        return result.stdout.decode()

    monkeypatch.setattr(oracle, "translate_vertex", translate_vertex)
    monkeypatch.setattr(oracle, "translate_pixel", translate_pixel)


@scene.gpu
def test_registers_and_export_records(graphics_backend):
    scene.test_vertex_shader_on_the_gpu_matches_the_interpreter()
    scene.test_pixel_shader_on_the_gpu_matches_the_interpreter()


@scene.gpu
def test_pixel_wave_ballots_and_multiple_render_targets(graphics_backend):
    wave.test_multi_lane_pixel_shader_lane_dependent_matches_interpreter(
        "fullscreen", [(-1.0, -1.0, 0.0), (3.0, -1.0, 2.5), (-1.0, 3.0, 1.0)])


@scene.gpu
def test_interpolation_moves_and_disabled_export_channels(graphics_backend, monkeypatch):
    shader = scene.program(*scene.vintrp("v_interp_mov_f32", 2, 2, 0, 0),
                           *scene.exp(0, (2, 2, 2, 2), enable=5))
    monkeypatch.setattr(scene, "PIXEL_SHADER", shader)
    scene.test_pixel_shader_on_the_gpu_matches_the_interpreter()
