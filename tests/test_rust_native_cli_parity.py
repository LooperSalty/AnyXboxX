"""Native CLI parity using only small PE files built from source by pe_builder."""
import os
import struct
import subprocess
import sys

import pytest

from pe_builder import Asm, build_pe
from test_perf import CSV
from test_rust_traduction_parity import SCARLETT, require_binary

pytestmark = pytest.mark.skipif(sys.platform != "win32", reason="Windows native loader")
BASE = 0x1_6800_0000
GET_ENV = ("kernel32.dll", "GetEnvironmentVariableA")
COPY = ("kernel32.dll", "CopyFileA")


def run_cli(rust, command, args):
    cli = [require_binary(), command] if rust else [sys.executable, SCARLETT, "--" + command]
    proc = subprocess.run([*cli, *args], env={**os.environ, "PYTHONIOENCODING": "utf-8"},
                          capture_output=True, encoding="utf-8", timeout=40)
    return proc.returncode, proc.stdout, proc.stderr


@pytest.mark.parametrize("fps", [False, True])
def test_target_and_fps_reach_the_native_process(tmp_path, runtime_dir, monkeypatch, fps):
    monkeypatch.delenv("ANYXBOXX_FPS", raising=False)

    def code(iat):
        return (Asm().frame().lea_rcx(128).lea_rdx(256).mov_r8d(8).call_iat(iat[GET_ENV])
                .unframe().ret().at(128).raw(b"ANYXBOXX_FPS\0").build())

    exe = build_pe(tmp_path / "fps.exe", code, imports=[GET_ENV], dll=False, base=BASE)
    args = [exe, "--fps"] if fps else [exe]
    python, rust = (run_cli(side, "natif", args) for side in (False, True))
    assert rust == python
    assert rust[0] == int(fps)
    assert "[natif] fps.exe chargé" in rust[1]
    assert "sans isolation" in rust[2]


def test_native_package_and_extra_dll_folder(tmp_path, runtime_dir):
    extra = tmp_path / "extra"
    extra.mkdir()
    build_pe(extra / "helper.dll", lambda _: Asm().mov_eax(57).ret().at(64).mov_eax(1).ret().build(),
             exports=[("Answer", 0)], entry=64)
    symbol = ("helper.dll", "Answer")
    build_pe(tmp_path / "answer.exe", lambda iat: Asm().frame().call_iat(iat[symbol]).unframe().ret().build(),
             imports=[symbol], dll=False, base=BASE)
    (tmp_path / "MicrosoftGame.config").write_text('<Game><ExecutableList><Executable Name="answer.exe" '
                                                    'TargetDeviceFamily="Scarlett"/></ExecutableList></Game>')
    python, rust = (run_cli(side, "natif", [tmp_path, extra]) for side in (False, True))
    assert rust == python
    assert rust[0] == 57


@pytest.mark.parametrize("entry,value", [("size", 512 * 1024 * 1024 + 1), ("clr", 0x1000)])
def test_native_rejects_unsupported_image_before_loading_dlls(tmp_path, entry, value):
    exe = build_pe(tmp_path / "invalid.exe", lambda _: Asm().ret().build(), dll=False)
    image = bytearray(exe.read_bytes())
    optional = struct.unpack_from("<I", image, 0x3c)[0] + 24
    struct.pack_into("<I", image, optional + (56 if entry == "size" else 112 + 14 * 8), value)
    exe.write_bytes(image)
    python, rust = (run_cli(side, "natif", [exe]) for side in (False, True))
    assert rust == python
    assert rust[0] == 1


# The four measured values put the displayed mean on a decimal rounding boundary.
ROUNDING = CSV.splitlines()[0] + "\n" + "".join(
    f"{second},60,{fps},16,18\n" for second, fps in enumerate([10, 60.02, 60.17, 59.95, 60.06], 1))


@pytest.mark.parametrize("samples,status", [(ROUNDING, 0), (CSV + "5,45,45,22,40\n", 2),
                                           (CSV.splitlines()[0] + "\n", 1), (None, 1)],
                         ids=["constant", "slow", "short", "absent"])
def test_native_benchmark_verdict_and_svg(tmp_path, runtime_dir, samples, status):
    # The fixture copies a known capture to the counter's requested path. No graphics device,
    # swap chain or live frame timing is involved. GetEnvironmentVariableA writes at most 512 bytes.
    def code(iat):
        return (Asm().frame().lea_rcx(128).lea_rdx(448).mov_r8d(512).call_iat(iat[GET_ENV])
                .lea_rcx(256).lea_rdx(448).zero_r8().call_iat(iat[COPY]).unframe().ret()
                .at(128).raw(b"ANYXBOXX_FPS_CSV\0").at(256).raw(b"sample.csv\0").build())

    exe = build_pe(tmp_path / "bench & chart.exe", code, imports=[GET_ENV, COPY], dll=False, base=BASE)
    if samples is not None:
        (tmp_path / "sample.csv").write_text(samples)
    charts = [tmp_path / "python.svg", tmp_path / "rust.svg"]
    python, rust = (run_cli(side, "banc-perf", [exe, "3", chart])
                    for side, chart in zip((False, True), charts))
    assert rust == python
    assert rust[0] == status, rust
    if status != 1:
        assert charts[0].read_bytes() == charts[1].read_bytes()
    else:
        assert not any(chart.exists() for chart in charts)
