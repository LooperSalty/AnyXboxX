"""Accès mémoire typés dans l'interpréteur RDNA 2 : buffer_load/store_format_* (MUBUF, format du V#)
et tbuffer_load/store_format_* (MTBUF, format de l'instruction), with their _d16 forms (16 bits per
component, two per VGPR: emu/rdna2_buffer_d16.py).

Formats et conversions : emu/rdna2_buffer_format.py. Adresse et bornes comme les autres accès
MUBUF (emu/rdna2_exec_memory.py, _buffer) : base + soffset + index × pas + décalage ; avec un pas
non nul, l'élément est dans le buffer si index < NUM_RECORDS, sinon si décalage + taille de
l'élément <= NUM_RECORDS (l'élément entier : ISA, section 8.1.5, p. 62, note 3 : les accès
load/store-format sont vérifiés « all or nothing »).
Hors du buffer, une lecture donne des composantes nulles et une écriture n'écrit rien.

Composantes lues (spécification XML d'AMD : « Load N-component formatted data … convert the data to
32 bit integral or floating point format ») :
- MUBUF : DST_SEL du V# pour chaque composante écrite : 0, 1 (1.0 ou 1, voir select_one) ou une
  composante X à W du format ; DST_SEL = 1 reste 1 hors du buffer (ISA, section 8.1.5, p. 62, note 1 : « except for
  components with V#.dst_sel = SEL_1 that return 1 »). Une sélection
  d'une composante absente du format, ou les valeurs 2 et 3, sont refusées.
- MTBUF : le format de l'instruction, composantes dans l'ordre, sans DST_SEL (ISA, section 8.1.4,
  table 31, p. 59 : TBUFFER_*_FORMAT_*, format « instruction », DST SEL « identity ») ; lire plus de
  composantes que le format n'en a est refusé. Un V# de FORMAT INVALID (0) désactive l'accès malgré le
  format de l'instruction (même page : « The resource's data format must not be INVALID; that format
  has specific meaning (unbound resource) ») : lecture nulle, rien d'écrit.
Écriture : autant de composantes que le format ; MUBUF exige un DST_SEL « identité » (voir
rdna2_buffer_format.store_selects_ok). Une adresse non alignée sur la taille d'une composante, lds,
tfe, SWIZZLE_ENABLE et ADD_TID_ENABLE sont refusés en les nommant.
"""
from emu import rdna2_buffer_d16 as d16, rdna2_buffer_format as formats
from emu.rdna2_exec_memory import NULL, descriptor

supports = formats.supports


def _untranslated(message):
    from emu.rdna2_exec import Untranslated

    return Untranslated(message)


def _resolve(name, access, words):
    """(format, accès autorisé par le V#) ; FormatError hors du cas pris en charge."""
    formats.check_descriptor(words)
    word3 = words[3]
    code = access.fixed if access.typed else (word3 >> formats.WORD3_FORMAT_SHIFT) & formats.FORMAT_MASK
    fmt = formats.format_for(code, access.store, access.d16)
    if access.typed and access.count > fmt.count:
        raise formats.FormatError(f"{name} : {access.count} composantes pour le format {fmt.name}")
    if access.store and access.count != fmt.count:
        raise formats.FormatError(f"{name} : {access.count} composantes écrites au format {fmt.name}")
    if not access.typed and access.store and not formats.store_selects_ok(fmt, word3):
        raise formats.FormatError(f"{name} : DST_SEL {formats.selects(word3)} non pris en charge en écriture")
    if not access.typed and not access.store:
        formats.check_load_selects(fmt, word3, access.count)
    enabled = not access.typed or (word3 >> formats.WORD3_FORMAT_SHIFT) & formats.FORMAT_MASK != 0
    return fmt, enabled


def execute(wave, name, word):
    """Exécute un accès typé ; `word` : les 64 bits de l'instruction."""
    try:
        access = formats.access(name, word)
        words = [wave.s[access.srsrc + i] for i in range(4)]
        fmt, enabled = _resolve(name, access, words)
    except formats.FormatError as error:
        raise _untranslated(str(error)) from None
    base, stride, records = descriptor(wave, access.srsrc)
    soffset = 0 if access.soffset == NULL else wave.read_scalar(access.soffset, None)
    for lane in wave.active():
        index = wave.v[access.vaddr][lane] if access.idxen else 0
        byte_offset = access.offset + (wave.v[access.vaddr + access.idxen][lane] if access.offen else 0)
        inside = enabled and (index < records if stride else byte_offset + fmt.size <= records)
        address = base + soffset + index * stride + byte_offset
        if inside and address % fmt.align:
            raise _untranslated(f"{name} : adresse 0x{address:X} non alignée pour le format {fmt.name}")
        if access.store:
            if inside:
                wave.memory.write(address, formats.encode(fmt, _store_values(wave, access, fmt, lane)))
            continue
        decode = d16.decode if access.d16 else formats.decode
        components = decode(fmt, wave.memory.read(address, fmt.size)) if inside else [0] * fmt.count
        results = _results(fmt, access, words[3], components)
        if access.d16:
            try:
                _write_halves(wave, access, lane, [d16.narrow(fmt, value) for value in results])
            except formats.FormatError as error:
                raise _untranslated(f"{name}: {error}") from None
            continue
        for i, value in enumerate(results):
            wave.v[access.vdata + i][lane] = value


def _store_values(wave, access, fmt, lane):
    """32-bit values to store; a d16 store widens the halves it reads (emu/rdna2_buffer_d16.py)."""
    if not access.d16:
        return [wave.v[access.vdata + i][lane] for i in range(fmt.count)]
    return [d16.widen(fmt, (wave.v[access.vdata + reg][lane] >> shift) & d16.HALF_MASK, width)
            for (reg, shift), width in zip(d16.slots(access), fmt.widths)]


def _write_halves(wave, access, lane, halves):
    """Each 16-bit result into its half of VGPR VDATA + i / 2; the other half is kept."""
    for (reg, shift), half in zip(d16.slots(access), halves):
        old = wave.v[access.vdata + reg][lane]
        wave.v[access.vdata + reg][lane] = old & ~(d16.HALF_MASK << shift) & formats.MASK32 | half << shift


def _results(fmt, access, word3, components):
    if access.typed:
        return components[:access.count]
    chosen = []
    for select in formats.selects(word3)[:access.count]:
        if select == formats.SELECT_ZERO:
            chosen.append(0)
        elif select == formats.SELECT_ONE:
            chosen.append(formats.select_one(fmt))
        else:
            chosen.append(components[select - formats.SELECT_X])
    return chosen
