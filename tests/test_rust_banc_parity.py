"""Parity of `anyxboxx banc` (crates/anyxboxx-report/src/bench.rs) with `scarlett.py --banc`.

Same folders of modules built from source by pe_builder. Module, imports and share resolved must be
the same on every row, and so must the footer's "imports résolus". The Python bench also loads and
initialises each module in the Unicorn sandbox; the sandbox is dropped (decision D2), so the Rust
table says `n/d` in those two columns and its footer says why, and never "oui".
"""
import os
import subprocess
import sys

import pytest

from pe_builder import Asm, build_pe
from test_rust_traduction_parity import SCARLETT, require_binary, write_registry

CODE = Asm().mov_eax(1).ret().build()
FOOTER = "imports résolus : "


@pytest.fixture(scope="module")
def binary():
    return require_binary()


@pytest.fixture(scope="module")
def registry(tmp_path_factory):
    return write_registry(tmp_path_factory.mktemp("registry") / "emulated.txt")


def run(command):
    environment = {**os.environ, "PYTHONIOENCODING": "utf-8"}
    done = subprocess.run(command, capture_output=True, text=True, encoding="utf-8", timeout=300, env=environment)
    assert done.returncode == 0, done.stderr
    return done.stdout


def rows(table):
    """[(module, imports, resolved, load, init, detail)] and the footer."""
    lines = table.strip().splitlines()
    cells = [tuple(cell.strip() for cell in line.strip("|").split("|")) for line in lines[2:] if line.startswith("|")]
    return cells, lines[-1]


def compare(binary, registry, folder, extra=()):
    python, python_footer = rows(run([sys.executable, str(SCARLETT), "--banc", str(folder), *map(str, extra)]))
    rust, rust_footer = rows(run([str(binary), "banc", str(folder), *map(str, extra), "--emulated-list",
                                  str(registry)]))
    assert [r[:3] for r in rust] == [p[:3] for p in python]
    assert rust_footer.split(FOOTER)[1] == python_footer.split(FOOTER)[1]
    for rust_row, python_row in zip(rust, python):
        if python_row[5].startswith("analyse : "):        # not analysed: Python does not load it either
            assert rust_row[3:5] == python_row[3:5] == ("non", "non")
            assert rust_row[5].startswith("analyse : ")
        else:
            assert rust_row[3:] == ("n/d", "n/d", "")
    assert "non disponibles en Rust" in rust_footer and "D2" in rust_footer
    return python, rust


def modules(folder):
    folder.mkdir(parents=True, exist_ok=True)
    build_pe(folder / "game.exe", lambda iat: CODE, dll=False, imports=[
        ("kernel32.dll", "ExitProcess"), ("xgameruntime.dll", "XGameRuntimeInitialize"), ("unknown.dll", "Foo"),
        ("helper.dll", "Seven"), ("xmem.dll", 12), ("vendor.dll", "V")])
    build_pe(folder / "helper.dll", lambda iat: CODE, exports=[("Seven", 0)],
             imports=[("kernel32.dll", "Sleep"), ("absent.dll", "A"), ("absent.dll", "B")])
    build_pe(folder / "Third.DLL", lambda iat: CODE, imports=[("api-ms-win-core-synch-l1-2-0.dll", "Sleep")])
    build_pe(folder / "none.dll", lambda iat: CODE)
    (folder / "notes.dll").write_bytes(b"not a PE")
    (folder / "x.exe").write_bytes(b"MZ junk")
    return folder


def test_rows_and_resolved_imports_equal_python(binary, registry, tmp_path):
    python, rust = compare(binary, registry, modules(tmp_path / "pkg"))
    assert [r[0] for r in rust] == ["game.exe", "helper.dll", "none.dll", "Third.DLL"]
    assert any(p[3] == "oui" for p in python) and not any("oui" in r for r in rust)


def test_extra_dll_folders_count_as_provided(binary, registry, tmp_path):
    vendor = tmp_path / "vendor"
    vendor.mkdir()
    build_pe(vendor / "vendor.dll", lambda iat: CODE, exports=[("V", 0)])
    compare(binary, registry, modules(tmp_path / "pkg"), extra=[vendor])


def test_module_that_cannot_be_analysed(binary, registry, tmp_path):
    folder = modules(tmp_path / "pkg")
    # A PE32+ header but no room for the optional header: refused by both analyses.
    (folder / "broken.dll").write_bytes(b"MZ" + bytes(0x3A) + (0x40).to_bytes(4, "little") + b"PE\0\0"
                                        + bytes(20) + (0x20B).to_bytes(2, "little") + bytes(16))
    python, rust = compare(binary, registry, folder)
    assert ("broken.dll", "0", "0 %", "non", "non") == rust[0][:5]


def test_empty_and_missing_folders(binary, registry, tmp_path):
    (tmp_path / "empty").mkdir()
    for folder in (tmp_path / "empty", tmp_path / "absent"):
        python, rust = compare(binary, registry, folder)
        assert python == rust == []
