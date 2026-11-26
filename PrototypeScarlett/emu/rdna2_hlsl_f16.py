"""Demi-précision (f16) dans le recompilateur HLSL, avec la sémantique de emu/rdna2_exec_f16.py.

Pas de types 16 bits (shader model 6.0, ni `half` ni f16tof32) : une valeur f16 est une moitié de VGPR (uint).
Chaque opération passe par le f32 dans une fonction `precise` (pas de x - x = 0 ni de 0 × x = 0), puis revient
en f16 par des routines entières, arrondi au plus proche pair. Somme, différence et produit de deux f16 sont
exacts en f32 ou n'y subissent qu'un arrondi sans effet sur le f16 (24 ≥ 2 × 11 + 2) ; le FMA arrondit d'abord
à l'impair (erreur exacte par TwoSum), puis en f16. Le pilote AMD (RX 9070 XT) replie pourtant `fadd x, 0.0`
en x, sans drapeau fast-math dans le DXIL : le signe d'un résultat exactement nul est donc fixé en entier.
"""
from emu.rdna2_exec_f16 import EXTENSIONS, INLINE_HALVES, OPERATIONS, VGPR_FIRST, WORD_RESULT
from emu.rdna2_hlsl_ops import Untranslatable, hexa, source

HELPERS = """uint F16ToF32(uint h) {
  uint s = (h & 0x8000u) << 16, e = (h >> 10) & 0x1Fu, m = h & 0x3FFu;
  if (e == 31u) return m != 0u ? 0x7FC00000u : s | 0x7F800000u;
  if (e != 0u) return s | ((e + 112u) << 23) | (m << 13);
  if (m == 0u) return s;
  uint p = firstbithigh(m);                       // dénormal f16 : normal en f32
  return s | ((p + 103u) << 23) | ((m << (23u - p)) & 0x7FFFFFu);
}
uint F32ToF16(uint f) {
  uint s = (f >> 16) & 0x8000u, a = f & 0x7FFFFFFFu;
  if (a > 0x7F800000u) return 0x7E00u;
  if (a >= 0x477FF000u) return s | 0x7C00u;
  if (a >= 0x38800000u) return s | ((a - 0x38000000u + 0xFFFu + ((a >> 13) & 1u)) >> 13);
  if (a < 0x33000000u) return s;
  uint shift = 126u - (a >> 23), m = (a & 0x7FFFFFu) | 0x800000u, q = m >> shift;
  uint rest = m & ((1u << shift) - 1u), tie = 1u << (shift - 1u);
  return s | (q + ((rest > tie || (rest == tie && (q & 1u) != 0u)) ? 1u : 0u));
}
uint F16Add(uint a, uint b) {
  precise float r = asfloat(F16ToF32(a)) + asfloat(F16ToF32(b));
  uint h = F32ToF16(asuint(r));
  return (h & 0x7FFFu) == 0u ? a & b & 0x8000u : h;    // somme exactement nulle : -0 seulement pour -0 + -0
}
uint F16Sub(uint a, uint b) { return F16Add(a, b ^ 0x8000u); }
uint F16Mul(uint a, uint b) {
  precise float r = asfloat(F16ToF32(a)) * asfloat(F16ToF32(b));
  return F32ToF16(asuint(r));
}
uint F16Fma(uint a, uint b, uint c) {
  precise float x = asfloat(F16ToF32(a)), y = asfloat(F16ToF32(b)), z = asfloat(F16ToF32(c));
  precise float p = x * y;                        // exact : deux mantisses de 11 bits
  precise float s = p + z;
  if ((asuint(s) & 0x7FFFFFFFu) == 0u) return (a ^ b) & c & 0x8000u;    // résultat exactement nul
  precise float t = s - p;
  precise float e = (p - (s - t)) + (z - t);      // erreur exacte de s (TwoSum)
  uint r = asuint(s);
  if ((r & 0x7F800000u) != 0x7F800000u && e != 0.0f && (r & 1u) == 0u)
    r += ((e > 0.0f) == (s > 0.0f)) ? 1u : 0xFFFFFFFFu;    // arrondi à l'impair
  return F32ToF16(r);
}
uint F16Ldexp(uint a, uint n) {
  int k = clamp(asint(n << 16) >> 16, -64, 64);   // au-delà de ±64, le f16 vaut déjà 0 ou l'infini
  precise float r = asfloat(F16ToF32(a)) * asfloat(uint(k + 127) << 23);
  return F32ToF16(asuint(r));
}
bool F16IsNan(uint h) { return (h & 0x7FFFu) > 0x7C00u; }
uint F16Key(uint h) { return (h & 0x8000u) != 0u ? ~h & 0xFFFFu : h | 0x8000u; }   // -0 juste sous +0
uint F16Min(uint a, uint b) {
  if (F16IsNan(a)) return F16IsNan(b) ? 0x7E00u : b;
  if (F16IsNan(b)) return a;
  return F16Key(a) <= F16Key(b) ? a : b;
}
uint F16Max(uint a, uint b) {
  if (F16IsNan(a)) return F16IsNan(b) ? 0x7E00u : b;
  if (F16IsNan(b)) return a;
  return F16Key(a) >= F16Key(b) ? a : b;
}
uint F16Clamp(uint h) { return F16IsNan(h) || (h & 0x8000u) != 0u ? 0u : min(h, 0x3C00u); }
uint F16Rndne(uint h) {                           // au plus proche pair, en entiers : pas de Round_ne (#227)
  uint s = h & 0x8000u, a = h & 0x7FFFu, e = a >> 10;
  if (a > 0x7C00u) return 0x7E00u;
  if (e >= 25u) return h;                         // |x| ≥ 1024 et ±infini : déjà entier
  if (e < 14u) return s;                          // |x| < 0.5
  if (e == 14u) return a == 0x3800u ? s : s | 0x3C00u;    // 0.5 ≤ |x| < 1 : 0.5 va à 0
  uint f = 25u - e, mask = (1u << f) - 1u, half = 1u << (f - 1u);    // bits sous l'unité
  uint t = a & ~mask, r = a & mask;
  bool up = r > half || (r == half && ((t >> f) & 1u) != 0u);
  return s | (t + (up ? 1u << f : 0u));           // la retenue passe dans l'exposant
}
uint F16Rcp(uint h) {
  uint s = h & 0x8000u, a = h & 0x7FFFu;
  if (a > 0x7C00u) return 0x7E00u;
  if (a == 0u) return s | 0x7C00u;
  if (a == 0x7C00u) return s;
  precise float r = 1.0f / asfloat(F16ToF32(h));
  return F32ToF16(asuint(r));
}
uint F16Sqrt(uint h) {
  uint a = h & 0x7FFFu;
  if (a > 0x7C00u) return 0x7E00u;
  if (a == 0u) return h;
  if ((h & 0x8000u) != 0u) return 0x7E00u;
  if (a == 0x7C00u) return 0x7C00u;
  precise float r = sqrt(asfloat(F16ToF32(h)));
  return F32ToF16(asuint(r));
}
uint F16Rsq(uint h) {
  uint s = h & 0x8000u, a = h & 0x7FFFu;
  if (a > 0x7C00u) return 0x7E00u;
  if (a == 0u) return s | 0x7C00u;
  if (s != 0u) return 0x7E00u;
  if (a == 0x7C00u) return 0u;
  precise float r = 1.0f / sqrt(asfloat(F16ToF32(h)));
  return F32ToF16(asuint(r));
}
uint F16Log(uint h) {
  uint a = h & 0x7FFFu;
  if (a > 0x7C00u) return 0x7E00u;
  if (a == 0u) return 0xFC00u;
  if ((h & 0x8000u) != 0u) return 0x7E00u;
  if (a == 0x7C00u) return 0x7C00u;
  if (h == 0x3C00u) return 0u;
  // Réduction x = 2^e * m avec m ∈ [√0.5, √2] puis polynôme impair en t = (m - 1)/(m + 1) : indépendant de log2() GPU.
  uint fb = F16ToF32(h);
  int e = int(fb >> 23) - 127;
  precise float m = asfloat((fb & 0x007FFFFFu) | 0x3F800000u);
  if (m > asfloat(0x3FB504F3u)) { m *= 0.5f; e += 1; }
  precise float t = (m - 1.0f) / (m + 1.0f);
  precise float t2 = t * t;
  precise float p = asfloat(0x3DE38E39u);          // 1/9
  p = p * t2 + asfloat(0x3E124925u);               // 1/7
  p = p * t2 + asfloat(0x3E4CCCCDu);               // 1/5
  p = p * t2 + asfloat(0x3EAAAAABu);               // 1/3
  p = p * t2 + 1.0f;
  precise float r = float(e) + (t * asfloat(0x4038AA3Bu)) * p;
  return F32ToF16(asuint(r));
}
uint F16Exp(uint h) {
  uint a = h & 0x7FFFu;
  if (a > 0x7C00u) return 0x7E00u;
  if (a == 0u) return 0x3C00u;
  if (h == 0xFC00u) return 0u;
  if (h == 0x7C00u) return 0x7C00u;
  precise float x = asfloat(F16ToF32(h));
  if (x >= 16.0f) return 0x7C00u;
  if (x <= -25.0f) return 0u;
  int n = int(floor(x + 0.5f));
  precise float f = x - float(n);
  // ln(2) scindé (13 bits hauts) : f * LN2_HI est exact en f32 pour toute mantisse f16 (11 bits).
  precise float y_hi = f * asfloat(0x3F317000u);
  precise float y_lo = f * asfloat(0x3805FDF4u);
  precise float y = y_hi + y_lo;
  precise float p = asfloat(0x39500D01u);          // 1/5040
  p = p * y + asfloat(0x3AB60B61u);                // 1/720
  p = p * y + asfloat(0x3C088889u);                // 1/120
  p = p * y + asfloat(0x3D2AAAABu);                // 1/24
  p = p * y + asfloat(0x3E2AAAABu);                // 1/6
  p = p * y + 0.5f;
  precise float rem = y_lo + (y * y) * p;
  // TwoSum + arrondi à l'impair (comme F16Fma) : évite le double arrondi (ex. 0x11C5 → 0x3C01).
  precise float s1 = 1.0f + y_hi;
  precise float t1 = s1 - 1.0f;
  precise float e1 = (1.0f - (s1 - t1)) + (y_hi - t1);
  precise float d2 = e1 + rem;
  precise float s = s1 + d2;
  precise float t = s - s1;
  precise float e = (s1 - (s - t)) + (d2 - t);
  uint r = asuint(s);
  if (e != 0.0f && (r & 1u) == 0u)
    r += (e > 0.0f) ? 1u : 0xFFFFFFFFu;
  precise float scaled = asfloat(r) * asfloat(uint(n + 127) << 23);
  return F32ToF16(asuint(scaled));
}
uint F16Trig(uint h, bool is_cos) {
  uint a = h & 0x7FFFu;
  if (a >= 0x7C00u) return 0x7E00u;
  if (a == 0u) return is_cos ? 0x3C00u : h;
  precise float x = abs(asfloat(F16ToF32(h)));
  bool neg = (h & 0x8000u) != 0u && !is_cos;
  precise float t = x - trunc(x);
  uint q = uint(floor(t * 4.0f));
  precise float u = t - float(q) * 0.25f;
  if (is_cos) q = (q + 1u) & 3u;
  if (u == 0.0f) {
    if ((q & 1u) == 0u) return 0u;
    return ((q == 3u) != neg) ? 0xBC00u : 0x3C00u;
  }
  if ((q & 1u) != 0u) u = 0.25f - u;
  // 2π scindé (11 bits hauts) + série de Taylor sur [0, π/4] : indépendant des approximations sin/cos GPU.
  precise float val = 0.0f;
  if (u > 0.125f) {
    precise float v = 0.25f - u;
    precise float th = v * asfloat(0x40C90000u) + v * asfloat(0x3AFDAA22u);
    precise float th2 = th * th;
    precise float p = asfloat(0x37D00D01u);        // 1/40320
    p = p * th2 - asfloat(0x3AB60B61u);            // -1/720
    p = p * th2 + asfloat(0x3D2AAAABu);            // 1/24
    p = p * th2 - 0.5f;
    val = 1.0f + th2 * p;
  } else {
    precise float th_hi = u * asfloat(0x40C90000u);
    precise float th_lo = u * asfloat(0x3AFDAA22u);
    precise float th = th_hi + th_lo;
    precise float th2 = th * th;
    precise float p = asfloat(0x3638EF1Du);        // 1/362880
    p = p * th2 - asfloat(0x39500D01u);            // -1/5040
    p = p * th2 + asfloat(0x3C088889u);            // 1/120
    p = p * th2 - asfloat(0x3E2AAAABu);            // -1/6
    val = th_hi + (th_lo + (th * th2) * p);
  }
  uint bits = F32ToF16(asuint(val));
  return (bits != 0u && ((q >= 2u) != neg)) ? (bits ^ 0x8000u) : bits;
}
"""

# Expression de chaque opération : {a} {b} {c} sources, {d} moitié basse de vdst avant l'instruction, {k} littéral.
TEMPLATES = {
    "v_add_f16": "F16Add({a}, {b})", "v_sub_f16": "F16Sub({a}, {b})", "v_subrev_f16": "F16Sub({b}, {a})",
    "v_mul_f16": "F16Mul({a}, {b})", "v_fmac_f16": "F16Fma({a}, {b}, {d})", "v_fmamk_f16": "F16Fma({a}, {k}, {b})",
    "v_fmaak_f16": "F16Fma({a}, {b}, {k})", "v_fma_f16": "F16Fma({a}, {b}, {c})", "v_max_f16": "F16Max({a}, {b})",
    "v_min_f16": "F16Min({a}, {b})", "v_ldexp_f16": "F16Ldexp({a}, {b})", "v_cvt_f16_f32": "F32ToF16({a})",
    "v_cvt_f32_f16": "F16ToF32({a})",
    # Entier d'un f16, exact en f32 et de retour en f16 ; signe gardé (IEEE), NaN → 0x7E00 par F32ToF16. Mêmes
    # fonctions HLSL que v_floor/ceil/trunc_f32 (rdna2_hlsl_ops). v_rndne_f16 est calculé en entiers : le pilote
    # NVIDIA 621.xx replie Round_ne(2.5) en 3.0 (#227).
    "v_floor_f16": "F32ToF16(asuint(floor(asfloat(F16ToF32({a})))))",
    "v_ceil_f16": "F32ToF16(asuint(ceil(asfloat(F16ToF32({a})))))",
    "v_trunc_f16": "F32ToF16(asuint(trunc(asfloat(F16ToF32({a})))))",
    "v_rndne_f16": "F16Rndne({a})",
    # Transcendantes f16 (ISA RDNA 2, §12.8, VOP1 84-88 et 96-97, p. 147-149).
    "v_rcp_f16": "F16Rcp({a})", "v_sqrt_f16": "F16Sqrt({a})", "v_rsq_f16": "F16Rsq({a})",
    "v_log_f16": "F16Log({a})", "v_exp_f16": "F16Exp({a})",
    "v_sin_f16": "F16Trig({a}, false)", "v_cos_f16": "F16Trig({a}, true)",
}


def _operand(name, kind, code, literal, high=False):
    """Source selon son type, comme l'interpréteur : mot f32 entier, ou 16 bits (moitié, constante, littéral)."""
    if kind == "w":
        return source(code, literal)
    if code < 128 or code >= VGPR_FIRST:                 # SGPR, VCC, M0, EXEC, NULL et VGPR
        return f"({source(code, literal)} >> 16)" if high else f"({source(code, literal)} & 0xFFFFu)"
    if high:
        raise Untranslatable(f"{name} : op_sel sur la moitié haute d'une constante non pris en charge")
    if code in INLINE_HALVES:
        if kind == "i":
            raise Untranslatable(f"{name} : constante flottante sur une source entière non prise en charge")
        return hexa(INLINE_HALVES[code])
    return f"({source(code, literal)} & 0xFFFFu)"         # constante entière ou littéral : 16 bits bas


def _written(name, d, expression, high):
    """Nouvelle valeur de v[d] : le résultat dans une moitié, l'autre gardée ; 32 bits pour v_cvt_f32_f16."""
    if name in WORD_RESULT:
        return expression
    if high:
        return f"(v[{d}] & 0xFFFFu) | ({expression} << 16)"
    return f"(v[{d}] & 0xFFFF0000u) | {expression}"


def vector(name, fields, literal, active):
    """VOP1/VOP2 f16 : sources et résultat dans la moitié basse."""
    if fields["src0"] in EXTENSIONS:
        raise Untranslatable(f"{name} : forme SDWA/DPP (src0 = 0x{fields['src0']:X}) non prise en charge en f16")
    kinds = OPERATIONS[name][0]
    codes = (fields["src0"], VGPR_FIRST + fields["vsrc1"]) if "vsrc1" in fields else (fields["src0"],)
    a, b = ([_operand(name, kind, code, literal) for kind, code in zip(kinds, codes)] + ["0u"])[:2]
    d = fields["vdst"]
    expression = TEMPLATES[name].format(a=a, b=b, c="0u", d=f"(v[{d}] & 0xFFFFu)", k=hexa((literal or 0) & 0xFFFF))
    return [f"if ({active}) v[{d}] = {_written(name, d, expression, False)};"]


def _check_vop3(name, kinds, fields, literal):
    """Formes VOP3 dont le comportement n'est pas sourcé : refusées comme dans l'interpréteur."""
    if name in WORD_RESULT and (fields.clamp or fields.omod):
        raise Untranslatable(f"{name} : clamp ou omod sur un résultat f32 non pris en charge")
    if fields.omod:
        raise Untranslatable(f"{name} : omod sur un résultat f16 non pris en charge")
    if fields.op_sel and name == "v_fmac_f16":
        raise Untranslatable(f"{name} : op_sel non pris en charge en VOP3")
    if fields.op_sel >> 3 & 1 and name in WORD_RESULT:
        raise Untranslatable(f"{name} : op_sel sur une destination de 32 bits non pris en charge")
    if name in ("v_fmamk_f16", "v_fmaak_f16") and literal is None:
        raise Untranslatable(f"{name} (VOP3) : sans littéral, la constante K n'existe pas")
    for index, kind in enumerate(kinds):
        if kind == "w" and fields.op_sel >> index & 1:
            raise Untranslatable(f"{name} : op_sel sur une source de 32 bits non pris en charge")
        if kind == "i" and (fields.abs | fields.neg) >> index & 1:
            raise Untranslatable(f"{name} : abs/neg sur une source entière non pris en charge")


def _modified(operand, kind, fields, index):
    """abs et neg de VOP3 : bit de signe de la moitié f16 (bit 15) ou du mot f32 (bit 31)."""
    if kind == "i":
        return operand
    if fields.abs >> index & 1:
        operand = f"({operand} & {'0x7FFFFFFFu' if kind == 'w' else '0x7FFFu'})"
    if fields.neg >> index & 1:
        operand = f"({operand} ^ {'0x80000000u' if kind == 'w' else '0x8000u'})"
    return operand


def vop3(name, fields, literal, active):
    """VOP3 f16 : op_sel (bits 0 à 2) lit la moitié haute d'une source, op_sel bit 3 écrit le résultat dans la
    moitié haute de vdst et garde la basse ; clamp borne le résultat f16 à [0, 1]."""
    kinds = OPERATIONS[name][0]
    _check_vop3(name, kinds, fields, literal)
    a, b, c = ([_modified(_operand(name, kind, code, literal, fields.op_sel >> index & 1), kind, fields, index)
                for index, (kind, code) in enumerate(zip(kinds, fields.sources))] + ["0u", "0u"])[:3]
    d = fields.vdst
    expression = TEMPLATES[name].format(a=a, b=b, c=c, d=f"(v[{d}] & 0xFFFFu)", k=hexa((literal or 0) & 0xFFFF))
    if fields.clamp:
        expression = f"F16Clamp({expression})"
    return [f"if ({active}) v[{d}] = {_written(name, d, expression, fields.op_sel >> 3 & 1)};"]
