"""Cache disque des shaders compilés (emu/shader_cache.py) et son usage par rdna2_gpu.compile_dxil."""
import time

import pytest

from emu import rdna2_gpu, rdna2_hlsl, shader_cache
from test_rdna2_exec import ONE, program, vop1

DXIL = b"DXBC" + bytes(60)


def test_round_trip_and_key_inputs(tmp_path):
    cache = shader_cache.ShaderCache(tmp_path)
    key = shader_cache.cache_key("hlsl", "cs_6_0", None)
    assert cache.get(key) is None
    cache.put(key, DXIL)
    assert cache.get(key) == DXIL and (cache.stats.hits, cache.stats.misses) == (1, 1)
    assert key != shader_cache.cache_key("hlsl ", "cs_6_0", None)
    assert key != shader_cache.cache_key("hlsl", "cs_6_6", None)


def test_compiler_update_changes_the_key(tmp_path):
    compiler = tmp_path / "dxc.exe"
    compiler.write_bytes(b"v1")
    before = shader_cache.cache_key("hlsl", "cs_6_0", compiler)
    compiler.write_bytes(b"version 2")
    assert shader_cache.cache_key("hlsl", "cs_6_0", compiler) != before


def test_corrupted_entries_are_dropped_and_non_dxil_refused(tmp_path):
    cache = shader_cache.ShaderCache(tmp_path)
    key = shader_cache.cache_key("x", "cs_6_0", None)
    cache.put(key, DXIL)
    path = next(tmp_path.glob("*/*.dxil"))
    path.write_bytes(b"garbage")
    fresh = shader_cache.ShaderCache(tmp_path)                    # autre processus : pas de couche mémoire
    assert fresh.get(key) is None and not path.exists()
    with pytest.raises(ValueError):
        cache.put(key, b"not dxil")


def test_oldest_entries_are_pruned(tmp_path):
    cache = shader_cache.ShaderCache(tmp_path, limit_bytes=3 * len(DXIL))
    keys = [shader_cache.cache_key(str(i), "cs_6_0", None) for i in range(5)]
    for key in keys:
        cache.put(key, DXIL)
        time.sleep(0.01)
    assert len(cache.entries()) == 3
    fresh = shader_cache.ShaderCache(tmp_path)
    assert fresh.get(keys[0]) is None and fresh.get(keys[-1]) == DXIL


def test_put_retries_transient_windows_permission_error_and_cleans_tmp(tmp_path, monkeypatch):
    cache = shader_cache.ShaderCache(tmp_path)
    key = shader_cache.cache_key("retry", "cs_6_0", None)
    real_replace = shader_cache.os.replace
    attempts = 0

    def flaky_replace(src, dst):
        nonlocal attempts
        attempts += 1
        if attempts < 3:
            raise PermissionError(5, "Access is denied")
        return real_replace(src, dst)

    monkeypatch.setattr(shader_cache.os, "replace", flaky_replace)
    monkeypatch.setattr(shader_cache.time, "sleep", lambda *_: None)
    cache.put(key, DXIL)
    assert attempts == 3
    assert cache.get(key) == DXIL
    assert list(tmp_path.glob("*/*.tmp")) == []


def test_put_accepts_identical_peer_entry_on_persistent_lock_else_raises(tmp_path, monkeypatch):
    cache = shader_cache.ShaderCache(tmp_path)
    key = shader_cache.cache_key("peer", "cs_6_0", None)
    cache.put(key, DXIL)

    def always_locked(_src, _dst):
        raise PermissionError(5, "Access is denied")

    monkeypatch.setattr(shader_cache.os, "replace", always_locked)
    monkeypatch.setattr(shader_cache.time, "sleep", lambda *_: None)
    # Identical content already written by a racing worker succeeds cleanly.
    cache.put(key, DXIL)
    assert list(tmp_path.glob("*/*.tmp")) == []

    # Existing destination with different bytes raises PermissionError, preserves
    # the existing peer bytes, and leaves no orphan .tmp.
    other_dxil = b"DXBC" + bytes([1] * 60)
    with pytest.raises(PermissionError):
        cache.put(key, other_dxil)
    assert next(tmp_path.glob("*/*.dxil")).read_bytes() == DXIL
    assert list(tmp_path.glob("*/*.tmp")) == []

    # Missing destination still raises PermissionError and leaves no orphan .tmp.
    missing_key = shader_cache.cache_key("missing", "cs_6_0", None)
    with pytest.raises(PermissionError):
        cache.put(missing_key, DXIL)
    assert list(tmp_path.glob("*/*.tmp")) == []


@pytest.mark.skipif(rdna2_gpu.find_dxc() is None, reason="dxc absent")
def test_second_compilation_comes_from_the_cache(tmp_path, monkeypatch):
    cache = shader_cache.ShaderCache(tmp_path)
    hlsl = rdna2_hlsl.translate(program(vop1("v_mov_b32", 1, ONE)))
    started = time.perf_counter()
    first = rdna2_gpu.compile_dxil(hlsl, cache)
    compile_time = time.perf_counter() - started
    dxc = rdna2_gpu.find_dxc()
    monkeypatch.setattr(rdna2_gpu, "compile_hlsl", lambda *_: pytest.fail("dxc rappelé malgré le cache"))
    monkeypatch.setattr(rdna2_gpu, "find_dxc", lambda: dxc)
    started = time.perf_counter()
    assert rdna2_gpu.compile_dxil(hlsl, cache) == first
    assert time.perf_counter() - started < compile_time
    assert first.startswith(b"DXBC") and cache.stats.hits == 1
