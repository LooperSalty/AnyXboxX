"""Resolve indexed registers before emission, using the interpreter's bounds and alignment."""
from emu.rdna2_exec import Untranslated
from emu.rdna2_exec_movrel import LAST_SGPR, LITERAL, M0, SCALAR, VECTOR, VGPR_FIRST, VGPRS, check, offsets
from emu.rdna2_hlsl_ops import NULL, Untranslatable, immediate, source, source_high

TRANSLATED = VECTOR | SCALAR


def next_m0(m0, family, name, word, literal):
    """Track unconditional immediate writes. Unknown writers invalidate the proof."""
    dest = (word >> 16) & 127
    if family in ("SOP1", "SOP2", "SOPK"):
        if dest == M0 and name == "s_mov_b32":
            return immediate(word & 255, literal)
        if dest == M0 and name == "s_movk_i32":
            value = word & 0xFFFF
            return ((value ^ 0x8000) - 0x8000) & 0xFFFFFFFF
        return None if dest in (M0 - 1, M0) else m0
    if family == "VOPC" and word & 511 == 0xF9:  # SDWA can select its own scalar destination.
        return None
    if family == "VOP3" and name not in VECTOR:
        # VOP3 can write a scalar result or mask at VDST or SDST.
        return None if any(d in (M0 - 1, M0) for d in (word & 255, word >> 8 & 127)) else m0
    if family == "SMEM":
        first = word >> 6 & 127
        return None if first <= M0 < first + 16 else m0
    if name in VECTOR or family in ("VOP2", "VOPC", "VOP3P", "SOPP", "SOPC", "DS", "MUBUF",
                                    "MTBUF", "MIMG", "GLOBAL", "FLAT", "SCRATCH", "EXP", "VINTRP"):
        return m0
    if family == "VOP1" and (name != "v_readfirstlane_b32" or (word >> 17) & 255 != M0):
        return m0
    return None


def incoming(data, instructions, names, initial, branches, branch_target):
    """A join keeps M0 only when every reachable predecessor supplies the same value."""
    if not TRANSLATED.intersection(names):
        return {}
    by_pc = {inst.offset: (inst, name) for inst, name in zip(instructions, names)}
    values = {0: initial}
    pending = [0]
    while pending:
        pc = pending.pop()
        inst, name = by_pc[pc]
        end = pc + 4 * inst.size
        word = int.from_bytes(data[pc:pc + 4], "little")
        literal = int.from_bytes(data[pc + 4:pc + 8], "little") if inst.size > 1 else None
        value = next_m0(values[pc], inst.family, name, word, literal)
        edges = [] if name in ("s_endpgm", "s_branch") else [end]
        if name in branches:
            edges.append(branch_target(data, inst))
        for target in edges:
            if target not in by_pc:
                continue
            if target in values:
                if values[target] is None or values[target] == value:
                    continue
                values[target] = None
            else:
                values[target] = value
            pending.append(target)
    return values


def _offsets(name, m0):
    if m0 is None and name != "v_swap_b32":
        raise Untranslatable(f"{name} : valeur de M0 inconnue à la traduction")
    return offsets(name, m0 or 0)


def _index(name, number, last, width=1):
    try:
        return check(name, number, last, width)
    except Untranslated as error:
        raise Untranslatable(str(error)) from error


def vector(name, vdst, src0, literal, active, m0):
    src_offset, dst_offset = _offsets(name, m0)
    dest = _index(name, vdst + dst_offset, VGPRS - 1)
    if name == "v_movreld_b32":
        return [f"if ({active}) v[{dest}] = {source(src0, literal)};"]
    if src0 < VGPR_FIRST:
        raise Untranslatable(f"{name} : la source indexée doit être un VGPR")
    src = _index(name, src0 - VGPR_FIRST + src_offset, VGPRS - 1)
    if name in ("v_swap_b32", "v_swaprel_b32"):
        return [f"if ({active}) {{ uint x = v[{src}]; v[{src}] = v[{dest}]; v[{dest}] = x; }}"]
    return [f"if ({active}) v[{dest}] = v[{src}];"]


def scalar(name, fields, literal, m0):
    src_offset, dst_offset = _offsets(name, m0)
    width = 2 if name.endswith("_b64") else 1
    src, dest = fields["ssrc0"], fields["sdst"]
    if name in ("s_movreld_b32", "s_movreld_b64"):
        if width == 2 and src == LITERAL:
            raise Untranslatable(f"{name} : littéral 64 bits")
        low = source(src, literal)
        high = source_high(src) if width == 2 and src != 127 else "0u"
    else:
        src = _index(name, src + src_offset, LAST_SGPR, width)
        low, high = f"s[{src}]", f"s[{src + 1}]"
    if name in ("s_movrels_b32", "s_movrels_b64"):
        if dest == NULL:
            return []
        if dest + width > 128:
            raise Untranslatable(f"{name} : destination au-delà de s127")
    else:
        dest = _index(name, dest + dst_offset, LAST_SGPR, width)
    if width == 1:
        return [f"s[{dest}] = {low};"]
    return [f"{{ uint x = {low}, y = {high}; s[{dest}] = x; s[{dest + 1}] = y; }}"]
