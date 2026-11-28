"""victory.exe exports D3D12SDKVersion and D3D12SDKPath in every x64 Windows MSVC build.

The D3D12 loader looks for these two exports in the executable; with them, a D3D12 Agility SDK
runtime in `D3D12\\` beside victory.exe is used instead of the system's. Windows 10's own runtime
refuses every root signature the title asks for, so the launcher's build needs them too: they are
on unless VICTORY_AGILITY_SDK=0, and a missing runtime folder falls back to the system's.
"""
import re
import struct
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "third_party" / "victory" / "apps" / "victory"
BUILD_RS = APP / "build.rs"
AGILITY_RS = APP / "src" / "agility.rs"
EXPORTS = ("D3D12SDKVersion", "D3D12SDKPath")
BUILT = [ROOT / "third_party" / "victory" / "target" / profile / "victory.exe" for profile in ("release", "diag")]


def _read(path):
    return path.read_text(encoding="utf-8")


def test_build_script_exports_both_names_unless_turned_off():
    script = _read(BUILD_RS)
    for name in EXPORTS:
        assert f"/EXPORT:{name},DATA" in script, name
    assert "CARGO_FEATURE_DIAGNOSTICS" not in script
    # Only "0" turns them off; an unset VICTORY_AGILITY_SDK keeps them on x64 MSVC.
    assert 'Ok("0") => return' in script
    assert '!= Ok("1")' not in script


def test_agility_module_defines_both_statics():
    source = _read(AGILITY_RS)
    for name in EXPORTS:
        assert re.search(r"#\[unsafe\(no_mangle\)\]\s*\n\s*static " + name + r"\b", source), name
    # Set at startup from the D3D12Core.dll beside victory.exe (the loader wants an exact match).
    assert "static D3D12SDKVersion: AtomicU32" in source
    assert "const TESTED: u32 = 620;" in source
    assert "D3D12SDKVersion.store(version, Relaxed);" in source
    assert 'SdkPath(c".\\\\D3D12\\\\".as_ptr())' in source


def test_d3d12_is_delay_loaded_so_the_version_set_in_main_is_read():
    script = _read(BUILD_RS)
    assert "/DELAYLOAD:d3d12.dll" in script
    assert "delayimp.lib" in script


def test_missing_runtime_stops_only_builds_that_require_it():
    source = _read(AGILITY_RS)
    required = source.index("#[cfg(agility_sdk_required)]")
    assert source.index("ensure!(", required) - required < 80


def _export_names(exe):
    """Names in the export directory of a PE32+ file (standard library only)."""
    data = exe.read_bytes()
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    sections = struct.unpack_from("<H", data, pe + 6)[0]
    optional_size = struct.unpack_from("<H", data, pe + 20)[0]
    optional = pe + 24
    export_rva = struct.unpack_from("<I", data, optional + 112)[0]
    if export_rva == 0:
        return set()
    table = optional + optional_size

    def offset(rva):
        for i in range(sections):
            entry = table + 40 * i
            size, address, _, raw = struct.unpack_from("<IIII", data, entry + 8)
            if address <= rva < address + max(size, 1):
                return rva - address + raw
        raise ValueError(f"RVA {rva:#x} outside every section")

    directory = offset(export_rva)
    # IMAGE_EXPORT_DIRECTORY: NumberOfNames at +24, AddressOfNames at +32.
    count = struct.unpack_from("<I", data, directory + 24)[0]
    names_rva = struct.unpack_from("<I", data, directory + 32)[0]
    names = set()
    for i in range(count):
        name_rva = struct.unpack_from("<I", data, offset(names_rva) + 4 * i)[0]
        start = offset(name_rva)
        names.add(data[start:data.index(b"\0", start)].decode("ascii"))
    return names


@pytest.mark.parametrize("exe", BUILT, ids=lambda path: path.parent.name)
def test_a_built_victory_exe_exports_both_names(exe):
    if not exe.is_file():
        pytest.skip(f"{exe} not built")
    assert set(EXPORTS) <= _export_names(exe)
