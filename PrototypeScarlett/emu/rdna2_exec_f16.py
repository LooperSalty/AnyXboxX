"""Demi-précision (f16) dans l'interpréteur de référence RDNA 2.

Une valeur f16 occupe une moitié de VGPR. Les opérations sont calculées en double précision puis
arrondies une seule fois en binary16, au plus proche pair : pour les sommes, produits et FMA de
valeurs f16, la double précision est exacte chaque fois que l'arrondi f16 en dépend. Les dénormaux
sont conservés (MODE par défaut) ; tout résultat NaN vaut 0x7E00, sans charge utile.
Sémantique d'après la documentation publique d'AMD (« RDNA 2 Instruction Set Architecture »).
"""
import math
import struct

from emu.rdna2_exec_ops import bits_float, float_bits, signed

HALF_NAN, HALF_INFINITY, HALF_ONE = 0x7E00, 0x7C00, 0x3C00
FLOAT_NAN = 0x7FC0_0000
# Constantes flottantes en ligne (codes 240 à 248) lues par une source f16 : leur encodage binary16.
INLINE_HALVES = {240: 0x3800, 241: 0xB800, 242: 0x3C00, 243: 0xBC00, 244: 0x4000, 245: 0xC000, 246: 0x4400,
                 247: 0xC400, 248: 0x3118}


def half_bits(value):
    """Bits binary16 de `value`, arrondi au plus proche pair ; ±infini au-delà du format."""
    if math.isnan(value):
        return HALF_NAN
    try:
        return struct.unpack("<H", struct.pack("<e", value))[0]
    except OverflowError:
        return HALF_INFINITY | (0x8000 if value < 0 else 0)


def bits_half(bits):
    return struct.unpack("<e", struct.pack("<H", bits & 0xFFFF))[0]


def _is_nan(bits):
    return bits & 0x7FFF > HALF_INFINITY


ALMOST_ONE = 0x3BFF                          # plus grand f16 sous 1.0


def half_fract(bits):
    """v_fract_f16 : S0 + -floor(S0) (ISA), au plus 0x3BFF comme v_fract_f32 ; NaN pour ±infini et NaN."""
    if _is_nan(bits) or bits & 0x7FFF == HALF_INFINITY:
        return HALF_NAN
    value = bits_half(bits)
    return half_bits(min(value - math.copysign(math.floor(value), value), bits_half(ALMOST_ONE)))


def half_frexp(bits):
    """(mantisse f16, exposant) de S0 = m × 2^e, 0.5 ≤ |m| < 1 (dénormaux normalisés) ; ±0, ±infini et NaN :
    S0 tel quel et exposant 0, comme v_frexp_*_f32."""
    if bits & 0x7FFF == 0 or bits & 0x7FFF >= HALF_INFINITY:
        return bits, 0
    mantissa, exponent = math.frexp(bits_half(bits))
    return half_bits(mantissa), exponent


def _order(bits):
    """Clé qui croît avec la valeur (NaN exclus), -0 juste sous +0."""
    return ~bits & 0xFFFF if bits & 0x8000 else bits | 0x8000


def half_min(a, b):
    """v_min_f16 : un NaN cède la place à l'autre opérande, -0 < +0."""
    if _is_nan(a):
        return HALF_NAN if _is_nan(b) else b
    if _is_nan(b):
        return a
    return a if _order(a) <= _order(b) else b


def half_max(a, b):
    if _is_nan(a):
        return HALF_NAN if _is_nan(b) else b
    if _is_nan(b):
        return a
    return a if _order(a) >= _order(b) else b


def half_clamp(bits):
    """clamp de VOP3 sur un résultat f16 : [0, 1], NaN et négatifs (-0 compris) donnent +0."""
    if _is_nan(bits) or bits & 0x8000:
        return 0
    return min(bits, HALF_ONE)


def half_round(bits, function):
    """v_floor/ceil/trunc/rndne_f16 : entier de S0 (exact en f16), signe gardé comme les formes f32
    (IEEE 754 roundToIntegral : trunc(-0.5) = -0) ; ±infini inchangé, NaN → 0x7E00."""
    if _is_nan(bits):
        return HALF_NAN
    if bits & 0x7FFF == HALF_INFINITY:
        return bits
    value = bits_half(bits)
    return half_bits(math.copysign(float(function(value)), value))
def half_toward_zero(value):
    """Bits binary16 de `value` arrondi vers zéro : l'arrondi au plus proche, reculé d'un pas s'il s'éloigne
    de zéro ; un fini trop grand donne ±65504, jamais l'infini ; NaN → 0x7E00."""
    if math.isnan(value) or math.isinf(value):
        return half_bits(value)
    bits = half_bits(value)
    return bits - 1 if abs(bits_half(bits)) > abs(value) else bits


def half_rcp(bits):
    """v_rcp_f16 (ISA RDNA 2, §12.8, VOP1 84, p. 147) : 1.0 / S0.f16 selon IEEE (±0 → ±inf, ±inf → ±0)."""
    sign, mag = bits & 0x8000, bits & 0x7FFF
    if mag > HALF_INFINITY:
        return HALF_NAN
    if mag == 0:
        return sign | HALF_INFINITY
    if mag == HALF_INFINITY:
        return sign
    return half_bits(1.0 / bits_half(bits))


def half_sqrt(bits):
    """v_sqrt_f16 (ISA RDNA 2, §12.8, VOP1 85, p. 147) : sqrt(S0.f16) (-0 → -0, négatif non nul → NaN)."""
    mag = bits & 0x7FFF
    if mag > HALF_INFINITY:
        return HALF_NAN
    if mag == 0:
        return bits
    if bits & 0x8000:
        return HALF_NAN
    if mag == HALF_INFINITY:
        return HALF_INFINITY
    return half_bits(math.sqrt(bits_half(bits)))


def half_rsq(bits):
    """v_rsq_f16 (ISA RDNA 2, §12.8, VOP1 86, p. 147) : 1.0 / sqrt(S0.f16) (±0 → ±inf, +inf → +0)."""
    sign, mag = bits & 0x8000, bits & 0x7FFF
    if mag > HALF_INFINITY:
        return HALF_NAN
    if mag == 0:
        return sign | HALF_INFINITY
    if sign:
        return HALF_NAN
    if mag == HALF_INFINITY:
        return 0
    return half_bits(1.0 / math.sqrt(bits_half(bits)))


def half_log2(bits):
    """v_log_f16 (ISA RDNA 2, §12.8, VOP1 87, p. 147) : log2(S0.f16) (±0 → -inf, +1 → +0, négatif → NaN)."""
    mag = bits & 0x7FFF
    if mag > HALF_INFINITY:
        return HALF_NAN
    if mag == 0:
        return 0x8000 | HALF_INFINITY
    if bits & 0x8000:
        return HALF_NAN
    if mag == HALF_INFINITY:
        return HALF_INFINITY
    if bits == HALF_ONE:
        return 0
    return half_bits(math.log2(bits_half(bits)))


def half_exp2(bits):
    """v_exp_f16 (ISA RDNA 2, §12.8, VOP1 88, p. 148) : 2 ** S0.f16 (-inf → +0, ±0 → +1, +inf → +inf)."""
    mag = bits & 0x7FFF
    if mag > HALF_INFINITY:
        return HALF_NAN
    if mag == 0:
        return HALF_ONE
    if bits == (0x8000 | HALF_INFINITY):
        return 0
    if bits == HALF_INFINITY:
        return HALF_INFINITY
    value = bits_half(bits)
    if value >= 16.0:
        return HALF_INFINITY
    if value <= -25.0:
        return 0
    return half_bits(2.0 ** value)


def half_trig(bits, is_cos):
    """v_sin_f16 / v_cos_f16 (ISA RDNA 2, §12.8, VOP1 96-97, p. 149) : sin/cos(S0.f16 × 2π), réduction exacte
    par tours avant la multiplication par 2π (sin(-0) = -0, sin(±65504) = +0, cos(±65504) = +1, cos(0.25) = +0)."""
    mag = bits & 0x7FFF
    if mag >= HALF_INFINITY:
        return HALF_NAN
    if mag == 0:
        return HALF_ONE if is_cos else bits
    value = abs(bits_half(bits))
    neg = bool(bits & 0x8000) and not is_cos
    frac = value - math.trunc(value)
    quadrant = int(math.floor(frac * 4.0))
    offset = frac - quadrant * 0.25
    if is_cos:
        quadrant = (quadrant + 1) & 3
    if offset == 0.0:
        if quadrant & 1 == 0:
            return 0
        return (0x8000 | HALF_ONE) if ((quadrant == 3) ^ neg) else HALF_ONE
    if quadrant & 1:
        offset = 0.25 - offset
    if offset > 0.125:
        result = math.cos((0.25 - offset) * math.tau)
    else:
        result = math.sin(offset * math.tau)
    encoded = half_bits(result)
    return encoded ^ 0x8000 if encoded and ((quadrant >= 2) ^ neg) else encoded


H = bits_half

# Opérations f16 : nom → (types des sources src0..src2, fonction(a, b, c, d, k)).
# Types : « h » moitié f16, « w » mot f32 de 32 bits, « i » entier de 16 bits. `d` : moitié de la
# destination avant l'instruction (accumulateur de fmac) ; `k` : 16 bits bas du littéral.
OPERATIONS = {
    "v_add_f16": ("hh", lambda a, b, c, d, k: half_bits(H(a) + H(b))),
    "v_sub_f16": ("hh", lambda a, b, c, d, k: half_bits(H(a) - H(b))),
    "v_subrev_f16": ("hh", lambda a, b, c, d, k: half_bits(H(b) - H(a))),
    "v_mul_f16": ("hh", lambda a, b, c, d, k: half_bits(H(a) * H(b))),
    "v_fmac_f16": ("hh", lambda a, b, c, d, k: half_bits(H(a) * H(b) + H(d))),
    "v_fmamk_f16": ("hh", lambda a, b, c, d, k: half_bits(H(a) * H(k) + H(b))),
    "v_fmaak_f16": ("hh", lambda a, b, c, d, k: half_bits(H(a) * H(b) + H(k))),
    "v_fma_f16": ("hhh", lambda a, b, c, d, k: half_bits(H(a) * H(b) + H(c))),
    "v_max_f16": ("hh", lambda a, b, c, d, k: half_max(a, b)),
    "v_min_f16": ("hh", lambda a, b, c, d, k: half_min(a, b)),
    # Au-delà de ±64, le résultat f16 vaut déjà 0 ou l'infini : borner l'exposant ne change rien.
    "v_ldexp_f16": ("hi", lambda a, b, c, d, k: half_bits(math.ldexp(H(a), max(-64, min(64, signed(b, 16)))))),
    # ISA RDNA 2, §12.8, opcodes VOP1 91-94 (p. 148) ; round() de Python : au plus proche pair.
    "v_floor_f16": ("h", lambda a, b, c, d, k: half_round(a, math.floor)),
    "v_ceil_f16": ("h", lambda a, b, c, d, k: half_round(a, math.ceil)),
    "v_trunc_f16": ("h", lambda a, b, c, d, k: half_round(a, math.trunc)),
    "v_rndne_f16": ("h", lambda a, b, c, d, k: half_round(a, round)),
    # ISA RDNA 2, §12.7, VOP2 47 (p. 135) : D.f16_lo = f32_to_f16(S0), D.f16_hi = f32_to_f16(S1), vers zéro.
    "v_cvt_pkrtz_f16_f32": ("ww", lambda a, b, c, d, k: half_toward_zero(bits_float(b)) << 16
                            | half_toward_zero(bits_float(a))),
    "v_cvt_f16_f32": ("w", lambda a, b, c, d, k: half_bits(bits_float(a))),
    "v_cvt_f32_f16": ("h", lambda a, b, c, d, k: FLOAT_NAN if _is_nan(a) else float_bits(H(a))),
    # ISA RDNA 2, §12.8, opcodes VOP1 89, 90 et 95 (p. 147-148) ; exposant d'un dénormal normalisé comme
    # le repli constant de LLVM (AMDGPUInstCombineIntrinsic.cpp, amdgcn_frexp_exp : frexp d'APFloat).
    "v_frexp_mant_f16": ("h", lambda a, b, c, d, k: half_frexp(a)[0]),
    "v_frexp_exp_i16_f16": ("h", lambda a, b, c, d, k: half_frexp(a)[1] & 0xFFFF),
    "v_fract_f16": ("h", lambda a, b, c, d, k: half_fract(a)),
    # ISA RDNA 2, §12.8, opcodes VOP1 84-88 et 96-97 (p. 147-149) : transcendantes f16 (0.51 ULP, dénormaux).
    "v_rcp_f16": ("h", lambda a, b, c, d, k: half_rcp(a)),
    "v_sqrt_f16": ("h", lambda a, b, c, d, k: half_sqrt(a)),
    "v_rsq_f16": ("h", lambda a, b, c, d, k: half_rsq(a)),
    "v_log_f16": ("h", lambda a, b, c, d, k: half_log2(a)),
    "v_exp_f16": ("h", lambda a, b, c, d, k: half_exp2(a)),
    "v_sin_f16": ("h", lambda a, b, c, d, k: half_trig(a, False)),
    "v_cos_f16": ("h", lambda a, b, c, d, k: half_trig(a, True)),
}
# Résultat sur 32 bits (le VGPR entier) ; les autres écrivent une moitié et gardent l'autre.
WORD_RESULT = frozenset({"v_cvt_f32_f16", "v_cvt_pkrtz_f16_f32"})
VGPR_FIRST = 256
EXTENSIONS = frozenset({0xE9, 0xEA, 0xF9, 0xFA})        # src0 de DPP8, DPP8_FI, SDWA, DPP16
INTEGER_RESULT = frozenset({"v_frexp_exp_i16_f16"})    # entier de 16 bits : le clamp f16 n'a pas de sens


def _refuse(message):
    from emu.rdna2_exec import Untranslated

    raise Untranslated(message)


def _source(wave, name, kind, code, lane, literal, high=False):
    """Source selon son type : mot f32 entier, ou 16 bits (moitié de registre, constante, littéral)."""
    if kind == "w":
        return wave.read_lane(code, lane, literal)
    if code < 128 or code >= VGPR_FIRST:                 # SGPR, VCC, M0, EXEC, NULL et VGPR
        word = wave.read_lane(code, lane, literal)
        return word >> 16 if high else word & 0xFFFF
    if high:
        _refuse(f"{name} : op_sel sur la moitié haute d'une constante non pris en charge")
    if code in INLINE_HALVES:
        if kind == "i":
            _refuse(f"{name} : constante flottante sur une source entière non prise en charge")
        return INLINE_HALVES[code]
    return wave.read_scalar(code, literal) & 0xFFFF      # constante entière ou littéral : 16 bits bas


def execute(wave, name, fields, literal):
    """VOP1/VOP2 f16 : sources et résultat dans la moitié basse ; la moitié haute de vdst est gardée."""
    if fields["src0"] in EXTENSIONS:
        _refuse(f"{name} : forme SDWA/DPP (src0 = 0x{fields['src0']:X}) non prise en charge en f16")
    kinds, operation = OPERATIONS[name]
    codes = (fields["src0"], VGPR_FIRST + fields["vsrc1"]) if "vsrc1" in fields else (fields["src0"],)
    destination = wave.v[fields["vdst"]]
    k = (literal or 0) & 0xFFFF
    for lane in wave.active():
        values = [_source(wave, name, kind, code, lane, literal) for kind, code in zip(kinds, codes)]
        a, b = (values + [0])[:2]
        old = destination[lane]
        result = operation(a, b, 0, old & 0xFFFF, k)
        destination[lane] = result if name in WORD_RESULT else (old & 0xFFFF_0000) | result


def _check_vop3(name, kinds, fields, literal):
    """Formes VOP3 dont le comportement n'est pas sourcé : refusées avant de toucher une ligne."""
    if name in WORD_RESULT and (fields.clamp or fields.omod):
        _refuse(f"{name} : clamp ou omod sur un résultat f32 non pris en charge")
    if fields.omod:
        _refuse(f"{name} : omod sur un résultat f16 non pris en charge")
    if fields.clamp and name in INTEGER_RESULT:
        _refuse(f"{name} : clamp sur un résultat entier non mesuré")
    if fields.op_sel and name == "v_fmac_f16":
        _refuse(f"{name} : op_sel non pris en charge en VOP3")
    if fields.op_sel >> 3 & 1 and name in WORD_RESULT:
        _refuse(f"{name} : op_sel sur une destination de 32 bits non pris en charge")
    if name in ("v_fmamk_f16", "v_fmaak_f16") and literal is None:
        _refuse(f"{name} (VOP3) : sans littéral, la constante K n'existe pas")
    for index, kind in enumerate(kinds):
        if kind == "w" and fields.op_sel >> index & 1:
            _refuse(f"{name} : op_sel sur une source de 32 bits non pris en charge")
        if kind == "i" and (fields.abs | fields.neg) >> index & 1:
            _refuse(f"{name} : abs/neg sur une source entière non pris en charge")


def _modified(value, kind, fields, index):
    """abs et neg de VOP3 : bit de signe de la moitié f16 (bit 15) ou du mot f32 (bit 31)."""
    if kind == "i":
        return value
    sign = 0x8000_0000 if kind == "w" else 0x8000
    if fields.abs >> index & 1:
        value &= ~sign
    if fields.neg >> index & 1:
        value ^= sign
    return value


def execute_vop3(wave, name, fields, literal):
    """VOP3 f16 : op_sel (bits 0 à 2) lit la moitié haute d'une source, op_sel bit 3 écrit le résultat dans la
    moitié haute de vdst et garde la basse ; clamp borne le résultat f16 à [0, 1]."""
    kinds, operation = OPERATIONS[name]
    _check_vop3(name, kinds, fields, literal)
    high = fields.op_sel >> 3 & 1
    destination = wave.v[fields.vdst]
    k = (literal or 0) & 0xFFFF
    for lane in wave.active():
        values = [_modified(_source(wave, name, kind, code, lane, literal, fields.op_sel >> index & 1), kind, fields, index)
                  for index, (kind, code) in enumerate(zip(kinds, fields.sources))]
        a, b, c = (values + [0, 0])[:3]
        old = destination[lane]
        result = operation(a, b, c, old & 0xFFFF, k)
        if fields.clamp:
            result = half_clamp(result)
        if name in WORD_RESULT:
            destination[lane] = result
        elif high:
            destination[lane] = (old & 0xFFFF) | result << 16
        else:
            destination[lane] = (old & 0xFFFF_0000) | result
