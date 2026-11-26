"""Parity of `anyxboxx traduction` (crates/anyxboxx-report) with `scarlett.py --traduction`.

Both commands run on the same package, runtime and TomBox folders built from source by pe_builder:
same table on stdout, byte-identical SVG, or both refuse. The Rust command receives the Python HLE
registry as its table of emulated functions and the Python RDNA 2 measure as its GPU table.

The RDNA 2 numbers come from Python code not ported yet (phase 4): the Rust binary embeds them from
crates/anyxboxx-report/data/rdna2-coverage.txt. `test_built_in_gpu_table_is_up_to_date` fails when
that table drifts from the Python measure; regenerate it with:

    python tests/test_rust_traduction_parity.py
"""
import os
import subprocess
import sys
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[1]
BINARY = REPO / "target" / "release" / ("anyxboxx.exe" if os.name == "nt" else "anyxboxx")
SCARLETT = REPO / "PrototypeScarlett" / "scarlett.py"
GPU_TABLE = REPO / "crates" / "anyxboxx-report" / "data" / "rdna2-coverage.txt"
GPU_HEADER = (
    "# RDNA 2 coverage per encoding family, measured by the Python oracle: encodings written with",
    "# their operands (emu/rdna2_coverage.by_family) and executed (emu/rdna2_exec.executable).",
    "# Regenerate: python tests/test_rust_traduction_parity.py",
    "# family disassembled executed total",
)


def gpu_coverage_table():
    """The table the Rust binary reads, measured by the Python disassembler and interpreter."""
    from emu.rdna2_coverage import by_family
    from emu.rdna2_exec import executable
    from emu.rdna2_ops import MNEMONICS

    executed = {}
    for (family, _), name in MNEMONICS.items():
        executed[family] = executed.get(family, 0) + executable(family, name)
    rows = (f"{family} {done} {executed[family]} {total}" for family, (done, total) in by_family().items())
    return "\n".join((*GPU_HEADER, *rows)) + "\n"


if __name__ == "__main__":
    sys.path[:0] = [str(REPO / "PrototypeScarlett"), str(REPO / "tests")]
    GPU_TABLE.write_text(gpu_coverage_table(), encoding="utf-8", newline="\n")
    print(f"written: {GPU_TABLE}")
    sys.exit(0)

from emu import hle  # noqa: E402
from pe_builder import Asm, build_pe  # noqa: E402

CODE = Asm().mov_eax(1).ret().build()
GAME_IMPORTS = [
    ("xmem.dll", "XMemAlloc"), ("xmem.dll", "XMemFree"), ("xmem.dll", "XMemGetWorkingSetStatistics"),
    ("d3d12_xs.dll", "D3D12SerializeVersionedRootSignature"), ("d3d12_xs.dll", "D3D12CreateDevice"),
    ("kernelx.dll", "Sleep"), ("kernelx.dll", 42), ("XGameRuntime.dll", "XGameRuntimeInitialize"),
    ("kernel32.dll", "GetTickCount"), ("unknown.dll", "Foo"), ("helper.dll", "Seven"),
]


def require_binary():
    """The Rust binary; absent: skipped, except in CI (ANYXBOXX_REQUIRE_RUNTIME=1)."""
    if not BINARY.is_file():
        if os.environ.get("ANYXBOXX_REQUIRE_RUNTIME") == "1":
            pytest.fail(f"{BINARY} absent: cargo build --release (root workspace).")
        pytest.skip("anyxboxx not built (cargo build --release at the root)")
    return BINARY


def write_registry(path):
    """The Python HLE registry as `family!function` lines: what emu/hle.resolve finds."""
    keys = set(hle.REGISTRY) | set(hle.GUEST_CODE)
    path.write_text("\n".join(f"{group}!{name}" for group, name in sorted(keys)) + "\n", encoding="utf-8")
    return path


@pytest.fixture(scope="module")
def binary():
    return require_binary()


@pytest.fixture(scope="module")
def registry(tmp_path_factory):
    return write_registry(tmp_path_factory.mktemp("registry") / "emulated.txt")


@pytest.fixture(scope="module")
def gpu_table(tmp_path_factory):
    path = tmp_path_factory.mktemp("gpu") / "rdna2-coverage.txt"
    path.write_text(gpu_coverage_table(), encoding="utf-8")
    return path


def environment(folders):
    return {**os.environ, "ANYXBOXX_RUNTIME": str(folders["runtime"]),
            "ANYXBOXX_TOMBOX_DIR": str(folders["tombox"]), "PYTHONIOENCODING": "utf-8"}


def run(command, folders, svg):
    """(exit code is 0, stdout, SVG bytes or None)."""
    done = subprocess.run(command, capture_output=True, text=True, encoding="utf-8", timeout=300,
                          env=environment(folders))
    image = svg.read_bytes() if svg is not None and svg.is_file() else None
    return done.returncode == 0, done.stdout, image, done.stderr


def python_run(folders, svg):
    command = [sys.executable, str(SCARLETT), "--traduction", str(folders["package"])]
    return run(command + ([str(svg)] if svg else []), folders, svg)


def rust_run(binary, folders, svg, *options):
    command = [str(binary), "traduction", str(folders["package"]), *map(str, options)]
    return run(command + ([str(svg)] if svg else []), folders, svg)


def make(tmp, *, game=GAME_IMPORTS, runtime=True, tombox=False, helper=True):
    package, rt, tb = tmp / "package", tmp / "runtime", tmp / "tombox"
    package.mkdir()
    build_pe(package / "game.exe", lambda iat: CODE, imports=game, dll=False)
    if helper:
        build_pe(package / "Helper.DLL", lambda iat: CODE, exports=[("Seven", 0)],
                 imports=[("xmem.dll", "XMemFree"), ("kernelx.dll", "Sleep"), ("xapu.dll", "XApuAlloc")])
        (package / "notes.dll").write_bytes(b"not a PE")
        (package / "broken.dll").write_bytes(b"MZ" + bytes(0x3A) + (0x40).to_bytes(4, "little") + b"PE\0\0"
                                             + bytes(20) + (0x20B).to_bytes(2, "little") + bytes(16))
    if runtime:
        rt.mkdir()
        build_pe(rt / "xmem.dll", lambda iat: CODE,
                 exports=[("XMemAlloc", 0), ("XMemGetWorkingSetStatistics", 0)])
        build_pe(rt / "kernelx.dll", lambda iat: CODE, exports=[("Sleep", "KERNEL32.Sleep")])
        build_pe(rt / "XGameRuntime.dll", lambda iat: CODE, exports=[("XGameRuntimeInitialize", 0)])
        (rt / "d3d12_xs.dll").write_bytes(b"unreadable")
    if tombox:
        tb.mkdir()
        build_pe(tb / "xmem.dll", lambda iat: CODE, exports=[("XMemFree", 0), ("XMemGetWorkingSetStatistics", 0)])
        build_pe(tb / "d3d12_xs.dll", lambda iat: CODE, exports=[("D3D12CreateDevice", 0)])
        build_pe(tb / "kernelx.dll", lambda iat: CODE, exports=[("Sleep", 0)])   # not preferred: ignored
    return {"package": package, "runtime": rt, "tombox": tb}


CASES = [
    ("runtime-only", lambda tmp: make(tmp)),
    ("runtime-and-tombox", lambda tmp: make(tmp, tombox=True)),
    ("no-runtime-folder", lambda tmp: make(tmp, runtime=False)),
    ("many-missing", lambda tmp: make(tmp, game=[("xmem.dll", f"XMemF{n}") for n in range(7)], helper=False)),
    ("no-console-import", lambda tmp: make(tmp, game=[("kernel32.dll", "Sleep")], helper=False)),
    ("no-imports", lambda tmp: make(tmp, game=[], helper=False)),
]


@pytest.mark.parametrize("make_case", [c[1] for c in CASES], ids=[c[0] for c in CASES])
def test_rust_table_and_svg_equal_python(binary, registry, gpu_table, tmp_path, make_case):
    folders = make_case(tmp_path)
    python = python_run(folders, tmp_path / "python.svg")
    rust = rust_run(binary, folders, tmp_path / "rust.svg", "--emulated-list", registry, "--gpu-coverage", gpu_table)
    assert python[0], python[3]
    assert rust[:3] == python[:3], rust[3]
    assert python[2].startswith(b"<svg")


def test_table_only_when_no_image_is_asked(binary, registry, gpu_table, tmp_path):
    folders = make(tmp_path)
    python = python_run(folders, None)
    assert rust_run(binary, folders, None, "--emulated-list", registry, "--gpu-coverage", gpu_table)[:3] == python[:3]


def test_both_refuse_an_image_in_a_missing_folder_after_the_table(binary, registry, gpu_table, tmp_path):
    folders = make(tmp_path)
    svg = tmp_path / "absent" / "traduction.svg"
    python = python_run(folders, svg)
    rust = rust_run(binary, folders, svg, "--emulated-list", registry, "--gpu-coverage", gpu_table)
    assert not python[0] and not rust[0]
    assert rust[1] == python[1] and "| Domaine |" in rust[1]
    assert rust[3].startswith("anyxboxx traduction: ")


def test_built_in_gpu_table_is_up_to_date():
    assert GPU_TABLE.read_text(encoding="utf-8") == gpu_coverage_table(), \
        "RDNA 2 coverage moved: python tests/test_rust_traduction_parity.py, then rebuild anyxboxx"


def test_default_gpu_table_is_the_built_in_one(binary, registry, tmp_path):
    folders = make(tmp_path)
    default = rust_run(binary, folders, None, "--emulated-list", registry)
    explicit = rust_run(binary, folders, None, "--emulated-list", registry, "--gpu-coverage", GPU_TABLE)
    assert default[0] and default[1] == explicit[1]


def test_malformed_gpu_table_is_refused(binary, tmp_path):
    folders = make(tmp_path)
    bad = tmp_path / "bad.txt"
    bad.write_text("SOP1 1 2\n", encoding="utf-8")
    ok, out, _, err = rust_run(binary, folders, None, "--gpu-coverage", bad)
    assert not ok and out == "" and err.startswith("anyxboxx traduction: GPU coverage table, line 1")


def test_default_emulated_table_is_victorys(binary, tmp_path):
    folders = make(tmp_path)
    ok, out, _, err = rust_run(binary, folders, None)
    assert ok, err
    assert out.startswith("| Domaine | Traduit | Total | Part | Reste |\n")
    assert "| System functions imported by the package |" in out
