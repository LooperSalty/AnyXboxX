"""Compare the shader subcommand and scarlett.py on identical, synthetic shader files."""
import os
import struct
import subprocess
import sys

import pytest

from emu import rdna2, rdna2_coverage
from emu.rdna2_ops import MNEMONICS
from test_rust_traduction_parity import SCARLETT, require_binary


def compare(path, options, code):
    environment = {**os.environ, "PYTHONIOENCODING": "utf-8"}
    results = [subprocess.run([*cli, path, *options], capture_output=True, text=True, encoding="utf-8",
                              env=environment, timeout=40)
               for cli in ([sys.executable, SCARLETT, "--rdna2"], [require_binary(), "rdna2"])]
    python, rust = results
    assert (rust.returncode, python.returncode) == (code, code), (rust.stderr, python.stderr)
    assert rust.stdout.splitlines() == python.stdout.splitlines()
    assert rust.stderr == python.stderr


def catalogue():
    for family, opcode in sorted(MNEMONICS | rdna2.ISA_ONLY_MNEMONICS):
        data = rdna2_coverage.encode(family, opcode)
        yield data[:rdna2.classify(data)[2] * 4]
        # Exercise register widths, literals, memory offsets, masks and packed modifiers.
        # Keep the opcode/prefix fields while varying registers and both instruction words.
        lo, hi = struct.unpack_from("<II", data)
        low_bits = {"SOP1": 0xff, "VOP1": 0x1ff, "MIMG": 0xbbfe}.get(family, 0xf57f)
        lo |= low_bits
        hi |= 0xdbaff72d
        variant = struct.pack("<6I", lo, hi, 0x807e7571, 0x3a291807, 0x348af0cd, 0x1234)
        yield variant[:rdna2.classify(variant)[2] * 4]


@pytest.mark.parametrize("options", [[], ["--liste"], ["--liste", "--executer"]])
def test_all_named_encodings_and_incomplete_tail(tmp_path, options):
    shader = tmp_path / "encodings.bin"
    # SDWA, DPP16, DPP8, literal, unknown word, truncated VOP3 and two trailing bytes.
    extra = bytes.fromhex("f9 fe ff 03 ff 0d 16 25 fa 02 0a 7e 01 01 01 00 e9 04 0a 06 01 88 c6 fa")
    shader.write_bytes(b"".join(catalogue()) + extra + struct.pack("<4I", 0xbe8003ff, 0xdeadbeef,
                                                                   0xffffffff, 0xd4000000) + b"ab")
    compare(shader, options, 0)


@pytest.mark.parametrize("words,code", [([], 0), ([0xbe820383, 0x7e080281, 0xbf810000], 0),
                                        ([0xffffffff], 1)])
def test_reference_wave_or_named_refusal(tmp_path, words, code):
    shader = tmp_path / "wave.bin"
    shader.write_bytes(struct.pack(f"<{len(words)}I", *words))
    compare(shader, ["--executer"], code)


def test_removed_spirv_precedes_file_read(tmp_path):
    compare(tmp_path / "absent.bin", ["--spirv", "--liste"], 2)


def test_reserved_sdwa_selector_is_refused(tmp_path):
    shader = tmp_path / "invalid.bin"
    shader.write_bytes(bytes.fromhex("f9 70 fe 7f 01 07 06 00"))
    # The Python operand printer raises IndexError. Rust reports the instruction by name.
    for cli in ([sys.executable, SCARLETT, "--rdna2"], [require_binary(), "rdna2"]):
        proc = subprocess.run([*cli, shader, "--liste"], capture_output=True, timeout=40)
        assert proc.returncode == 1 and not proc.stdout
        assert (b"IndexError" if cli[0] == sys.executable else b"v_bfrev_b32_sdwa") in proc.stderr
