"""VIctory copied into third_party/victory, and the settings `anyxboxx run` gives it.

`anyxboxx run` (crates/anyxboxx/src/run/settings.rs) replaced tools/lancer_victory.ps1 after a parity test
against it; its behaviour (calls, environment, exit codes) is checked in tests/test_anyxboxx_run.py.
"""
import configparser
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
VICTORY = ROOT / "third_party" / "victory"
SETTINGS = ROOT / "crates" / "anyxboxx" / "src" / "run" / "settings.rs"
GOLDEN = ROOT / "tests" / "fixtures" / "lancer_victory_neptune.ps1"
# The tuned set of main's script (Neptune base), read from its golden copy.
TUNED = tuple(re.findall(r"(VICTORY_[A-Z0-9_]+)\s*=\s*'",
                         GOLDEN.read_text(encoding="utf-8").split("$tuned = [ordered]@{")[1].split("\n    }")[0]))
# Absent from VIctory's code at the pinned commit: setting them changes nothing.
ABSENT = (
    "VICTORY_RT_MISS", "VICTORY_WAVE32_READFIRSTLANE", "VICTORY_WAVE32_CROSSLANE", "VICTORY_SKIP_BVH_BUILDER",
    "VICTORY_CAPPED_DYNAMIC",
)


def _settings():
    return SETTINGS.read_text(encoding="utf-8")


def _variables_set():
    """VICTORY_* names written by `anyxboxx run`."""
    return set(re.findall(r'"(VICTORY_[A-Z0-9_]+)"', _settings()))


def _tuned_table():
    text = _settings()
    return text[text.index("pub const TUNED"):text.index("];", text.index("pub const TUNED"))]


def test_gitmodules_no_longer_declares_victory():
    declared = configparser.ConfigParser()
    declared.read(ROOT / ".gitmodules", encoding="utf-8")
    assert all(declared[s]["path"] != "third_party/victory" for s in declared.sections())


def test_victory_source_is_tracked_in_the_repository():
    assert (VICTORY / "Cargo.toml").is_file() and (VICTORY / "LICENSE").is_file()
    output = subprocess.run(["git", "ls-files", "-s", "third_party/victory"], cwd=ROOT, capture_output=True,
                            text=True, check=False).stdout
    assert not any(line.startswith("160000 ") for line in output.splitlines())   # no gitlink


def test_the_tuned_settings_are_the_measured_ones():
    table = _tuned_table()
    assert set(re.findall(r'"(VICTORY_[A-Z0-9_]+)"', table)) == set(TUNED)


def test_dred_and_xg_are_never_tuned():
    # DRED and the XG library are opt-in (--dred, --xg-dll). The relay is part of main's golden set
    # (VICTORY_RELAY=2), not a switch of ours.
    table = _tuned_table()
    assert "DRED" not in table and "XG" not in table
    assert ("VICTORY_RELAY", "2") in re.findall(r'\("(VICTORY_[A-Z0-9_]+)", "([^"]*)"\)', table)


def test_no_variable_victory_does_not_read():
    assert not _variables_set() & set(ABSENT)


def test_every_variable_anyxboxx_run_sets_is_read_by_victory():
    code = "\n".join(f.read_text(encoding="utf-8", errors="replace") for f in VICTORY.rglob("*.rs")
                     if "target" not in f.relative_to(VICTORY).parts)
    for name in sorted(_variables_set() - {"VICTORY_GAME_DIR", "VICTORY_SKIP_PIPELINES"}):
        assert f'"{name}"' in code, name


def test_no_xg_library_in_the_repository():
    # Rule 1 of AGENTS.md: the GDK's xg_xs.dll comes from the user's copy, never from the repository.
    output = subprocess.run(["git", "ls-files", "*.dll"], cwd=ROOT, capture_output=True, text=True,
                            check=False).stdout
    assert "xg_xs" not in output.lower()
