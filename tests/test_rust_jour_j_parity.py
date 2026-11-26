"""Parity of `anyxboxx jour-j` (crates/anyxboxx-report/src/jour_j.rs) with `tools/jour_j.py`.

Same package built from source by pe_builder, same runtime, TomBox and System32 folders (temporary
ones): the static part of rapport.md (inventory and providers) must be byte-identical and the summary
printed the same, or both refuse. The launch steps differ on purpose: the Unicorn sandbox and
anyxboxx.exe are dropped (decisions D2 and D4), so the Rust report runs `anyxboxx rapport` for the
analysis and says the two launches are not available. The Python side runs in-process with its
launch steps stubbed out, so no game code nor sandbox is involved.
"""
import os
import subprocess
import sys
from pathlib import Path

import pytest

from pe_builder import Asm, build_pe
from test_rust_traduction_parity import require_binary

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
import jour_j  # noqa: E402

CODE = Asm().mov_eax(7).ret().build()
MANIFEST = ('<Game><ExecutableList><Executable Name="{exe}" TargetDeviceFamily="Scarlett" />'
            '</ExecutableList></Game>')
IMPORTS = [
    ("xmem.dll", "XMemAlloc"), ("xmem.dll", "XMemFree"), ("kernel32.dll", "GetTickCount"),
    ("KERNEL32.dll", "Sleep"), ("inconnu.dll", "Rien"), ("helper.dll", "Seven"), ("helper.dll", "Absent"),
    ("helper.dll", 3), ("api-ms-win-core-synch-l1-2-0.dll", "WaitForSingleObjectEx"),
    ("d3d12_xs.dll", "D3D12CreateDevice"), ("ntdll.dll", "RtlZeroMemory"), ("ucrtbase.dll", "malloc"),
    ("xgameruntime.dll", "XGameRuntimeInitialize"),
]


@pytest.fixture(scope="module")
def binary():
    return require_binary()


def folders(tmp):
    """Package, runtime, TomBox and SystemRoot folders; System32 holds a fake kernel32 and ntdll."""
    package, runtime, tombox, system = tmp / "paquet", tmp / "runtime", tmp / "tombox", tmp / "windows"
    for folder in (package, runtime, tombox, system / "System32"):
        folder.mkdir(parents=True)
    build_pe(package / "helper.dll", lambda iat: CODE, exports=[("Seven", 0)])
    build_pe(runtime / "xmem.dll", lambda iat: CODE, exports=[("XMemAlloc", 0)])
    build_pe(tombox / "xmem.dll", lambda iat: CODE, exports=[("XMemFree", 0)])
    build_pe(tombox / "d3d12_xs.dll", lambda iat: CODE, exports=[("Other", 0)])
    build_pe(system / "System32" / "kernel32.dll", lambda iat: CODE,
             exports=[("GetTickCount", 0), ("Sleep", "KERNELBASE.Sleep")])
    (system / "System32" / "ntdll.dll").write_bytes(b"not a PE")
    return {"package": package, "runtime": runtime, "tombox": tombox, "system": system}


def game(tmp, imports=IMPORTS, manifest=True, name="game.exe", **options):
    where = folders(tmp)
    build_pe(where["package"] / name, lambda iat: CODE, imports=imports, dll=False, **options)
    if manifest:
        (where["package"] / "MicrosoftGame.config").write_text(MANIFEST.format(exe=name), encoding="utf-8")
    return where


def python_report(where, target, output, monkeypatch, capsys):
    monkeypatch.setattr(jour_j, "RUNTIME", where["runtime"])
    monkeypatch.setattr(jour_j, "TOMBOX", where["tombox"])
    monkeypatch.setattr(jour_j, "SYSTEM32", where["system"] / "System32")
    monkeypatch.setattr(jour_j, "isolated_steps", lambda target, output: [])
    monkeypatch.setattr(jour_j, "_EXPORTS", {})
    capsys.readouterr()
    code = jour_j.main([str(target), "--sortie", str(output)])
    report = output / "rapport.md"
    return code, capsys.readouterr().out, report.read_bytes() if report.is_file() else None


def rust_report(binary, where, target, output, *options):
    environment = {**os.environ, "ANYXBOXX_RUNTIME": str(where["runtime"]),
                   "ANYXBOXX_TOMBOX_DIR": str(where["tombox"]), "SystemRoot": str(where["system"])}
    done = subprocess.run([str(binary), "jour-j", str(target), "--sortie", str(output), *map(str, options)],
                          capture_output=True, text=True, encoding="utf-8", timeout=120, env=environment)
    report = output / "rapport.md"
    return done.returncode, done.stdout, report.read_bytes() if report.is_file() else None, done.stderr


CASES = [
    ("package", lambda tmp: (game(tmp), "package")),
    ("executable", lambda tmp: (game(tmp, manifest=False), "exe")),
    ("tls", lambda tmp: (game(tmp, pointers={0x80: 0x10}, tls=0x40), "package")),
    ("no-imports", lambda tmp: (game(tmp, imports=[]), "package")),
    ("many-missing", lambda tmp: (game(tmp, imports=[("absent.dll", f"F{n}") for n in range(30)]
                                       + [("autre.dll", "G"), ("absent.dll", 9)]), "package")),
]


def target_of(where, kind):
    return where["package"] if kind == "package" else where["package"] / "game.exe"


@pytest.mark.parametrize("make", [c[1] for c in CASES], ids=[c[0] for c in CASES])
def test_static_report_equals_python(binary, tmp_path, monkeypatch, capsys, make):
    where, kind = make(tmp_path)
    target = target_of(where, kind)
    code, out, python = python_report(where, target, tmp_path / "py", monkeypatch, capsys)
    assert code == 0 and python is not None
    rust = rust_report(binary, where, target, tmp_path / "rs")
    assert rust[0] == 0, rust[3]
    assert rust[2].startswith(python), rust[2].decode("utf-8")
    assert rust[1] == out.replace(str(tmp_path / "py"), str(tmp_path / "rs"))
    rest = rust[2][len(python):].decode("utf-8")
    assert rest.startswith("## Analyse (anyxboxx rapport)")


def test_rust_launch_steps_say_they_are_not_available(binary, tmp_path):
    where = game(tmp_path)
    code, out, report, err = rust_report(binary, where, where["package"], tmp_path / "rs", "--natif")
    text = report.decode("utf-8")
    assert code == 0
    analysis = text.split("## Analyse (anyxboxx rapport)")[1].split("## ")[0]
    assert "code de sortie 0" in analysis and "Executable: game.exe" in analysis
    isolated = text.split("## Lancement isolé dans Unicorn (--lancer)")[1].split("## ")[0]
    assert "Non disponible" in isolated and "D2" in isolated and "code de sortie" not in isolated
    native = text.split("## Lancement natif (anyxboxx.exe)")[1]
    assert "Non disponible" in native and "anyxboxx run" in native and "code de sortie" not in native
    assert "--natif is not available" in err


def test_native_section_only_with_the_option(binary, tmp_path):
    where = game(tmp_path)
    _, _, report, err = rust_report(binary, where, where["package"], tmp_path / "rs")
    assert b"Lancement natif" not in report and err == ""


REFUSED = [
    ("missing-executable", lambda tmp: game(tmp, name="other.exe") and (
        (tmp / "paquet" / "MicrosoftGame.config").write_text(MANIFEST.format(exe="absent.exe"), encoding="utf-8")
        and tmp / "paquet")),
    ("folder-without-manifest", lambda tmp: game(tmp, manifest=False) and tmp / "paquet"),
    ("not-a-pe", lambda tmp: folders(tmp) and (tmp / "x.exe").write_bytes(b"MZ junk") and tmp / "x.exe"),
    ("absent-file", lambda tmp: folders(tmp) and tmp / "absent.exe"),
]


@pytest.mark.parametrize("make", [c[1] for c in REFUSED], ids=[c[0] for c in REFUSED])
def test_both_refuse(binary, tmp_path, monkeypatch, capsys, make):
    target = make(tmp_path)
    where = {"runtime": tmp_path / "runtime", "tombox": tmp_path / "tombox", "system": tmp_path / "windows"}
    code, _, python = python_report(where, target, tmp_path / "py", monkeypatch, capsys)
    rust = rust_report(binary, where, target, tmp_path / "rs")
    assert code == 1 and python is None
    assert rust[0] == 1 and rust[2] is None and rust[3].startswith("anyxboxx jour-j: ")
    assert (tmp_path / "rs").is_dir()          # the report folder is made first, as in Python


def test_no_target_is_a_usage_error(binary):
    done = subprocess.run([str(binary), "jour-j"], capture_output=True, text=True, timeout=60)
    assert done.returncode == jour_j.main([]) == 2
