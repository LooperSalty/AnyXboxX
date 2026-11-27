"""Fonctions HLSL partagées par les traductions RDNA 2 (emu/rdna2_hlsl_ops.py les réexporte).

Texte HLSL seulement : multiplications hautes, champs de bits, conversions saturées, masques de
quads, classes IEEE, permutations d'octets… Chaque fonction est vérifiée contre l'interpréteur de
référence par les tests de parité GPU qui l'utilisent.
"""
from emu import rdna2_hlsl_fma

HELPERS = """uint MulHiU32(uint a, uint b) {
  uint a0 = a & 0xFFFFu, a1 = a >> 16, b0 = b & 0xFFFFu, b1 = b >> 16;
  uint p00 = a0 * b0, p01 = a0 * b1, p10 = a1 * b0, p11 = a1 * b1;
  uint mid = (p00 >> 16) + (p01 & 0xFFFFu) + (p10 & 0xFFFFu);
  return p11 + (p01 >> 16) + (p10 >> 16) + (mid >> 16);
}
uint MulHiI32(uint a, uint b) {
  return MulHiU32(a, b) - (asint(a) < 0 ? b : 0u) - (asint(b) < 0 ? a : 0u);
}
uint BfeI32(uint v, uint off, uint w) {
  w &= 31u;
  if (w == 0u) return 0u;
  int field = asint(v) >> (off & 31u);
  uint shift = 32u - w;
  return asuint((field << shift) >> shift);
}
uint SatF32ToI32(uint a) {
  float x = asfloat(a);
  if ((a & 0x7FFFFFFFu) > 0x7F800000u) return 0u;          // NaN : test sur les bits, pas x != x
  if (x >= 2147483648.0f) return 0x7FFFFFFFu;
  if (x <= -2147483648.0f) return 0x80000000u;
  return asuint((int)x);
}
uint SatF32ToU32(uint a) {
  float x = asfloat(a);
  if ((a & 0x7FFFFFFFu) > 0x7F800000u || x <= 0.0f) return 0u;
  return x >= 4294967296.0f ? 0xFFFFFFFFu : (uint)x;
}
uint FloorToI32(uint a, bool nearest) {
  // v_cvt_flr/rpi_i32_f32 (mesures RDNA 2, VIctory #605) : NaN selon le signe ; rpi = floor(x + 0.5)
  // exact, obtenu en comparant la fraction x - floor(x) (exacte en simple précision) à 0.5.
  if ((a & 0x7FFFFFFFu) > 0x7F800000u) return (a >> 31) != 0u ? 0x80000000u : 0x7FFFFFFFu;
  float x = asfloat(a), f = floor(x);
  if (nearest && x - f >= 0.5f) f += 1.0f;
  return SatF32ToI32(asuint(f));
}
uint SBfeU32(uint v, uint c) {
  uint w = (c >> 16) & 0x7Fu;
  if (w == 0u) return 0u;
  uint field = v >> (c & 31u);
  return w >= 32u ? field : field & ((1u << w) - 1u);
}
uint SBfeI32(uint v, uint c) {
  uint w = (c >> 16) & 0x7Fu;
  if (w == 0u) return 0u;
  int field = asint(v) >> (c & 31u);
  if (w >= 32u) return asuint(field);
  uint shift = 32u - w;
  return asuint((field << shift) >> shift);
}
uint WqmB32(uint v) {
  uint r = 0u;
  [unroll] for (uint q = 0u; q < 32u; q += 4u) { if (((v >> q) & 0xFu) != 0u) r |= 0xFu << q; }
  return r;
}
uint QuadMaskB32(uint v) {
  uint r = 0u;
  [unroll] for (uint q = 0u; q < 8u; ++q) { if (((v >> (q * 4u)) & 0xFu) != 0u) r |= 1u << q; }
  return r;
}
uint LeadingSignsI32(uint v) {
  uint x = (v & 0x80000000u) != 0u ? ~v : v;
  return x == 0u ? 0xFFFFFFFFu : 31u - firstbithigh(x);
}
uint FrexpExpF32(uint a) {
  uint e = (a >> 23) & 0xFFu, m = a & 0x7FFFFFu;
  if (e == 0xFFu || (e == 0u && m == 0u)) return 0u;
  return e != 0u ? e - 126u : asuint((int)firstbithigh(m) - 148);
}
uint FrexpMantF32(uint a) {
  uint e = (a >> 23) & 0xFFu, m = a & 0x7FFFFFu;
  if (e == 0xFFu || (e == 0u && m == 0u)) return a;
  if (e == 0u) m = (m << (23u - firstbithigh(m))) & 0x7FFFFFu;
  return (a & 0x80000000u) | (126u << 23) | m;
}
bool IsNanF32(uint a) {
  return (a & 0x7FFFFFFFu) > 0x7F800000u;
}
uint ClassF32(uint a) {
  uint e = (a >> 23) & 0xFFu, m = a & 0x7FFFFFu, n = a >> 31;
  if (e == 0xFFu) return m != 0u ? ((m >> 22) & 1u) : (n != 0u ? 2u : 9u);
  if (e == 0u) return m == 0u ? (n != 0u ? 5u : 6u) : (n != 0u ? 4u : 7u);
  return n != 0u ? 3u : 8u;
}
uint SadU8(uint a, uint b) {
  uint r = 0u;
  [unroll] for (uint i = 0u; i < 32u; i += 8u) {
    uint x = (a >> i) & 0xFFu, y = (b >> i) & 0xFFu;
    r += x > y ? x - y : y - x;
  }
  return r;
}
uint LerpU8(uint a, uint b, uint c) {
  uint r = 0u;
  [unroll] for (uint i = 0u; i < 32u; i += 8u)
    r |= ((((a >> i) & 0xFFu) + ((b >> i) & 0xFFu) + ((c >> i) & 1u)) >> 1) << i;
  return r;
}
float4 Cube(uint a, uint b, uint c) {
  float x = asfloat(a), y = asfloat(b), z = asfloat(c);
  if (abs(z) >= abs(x) && abs(z) >= abs(y)) return float4(z < 0.0f ? 5.0f : 4.0f, z < 0.0f ? -x : x, -y, 2.0f * z);
  if (abs(y) >= abs(x)) return float4(y < 0.0f ? 3.0f : 2.0f, x, y < 0.0f ? -z : z, 2.0f * y);
  return float4(x < 0.0f ? 1.0f : 0.0f, x < 0.0f ? z : -z, -y, 2.0f * x);
}
uint AlignBit(uint hi, uint lo, uint s) {
  s &= 31u;
  return s == 0u ? lo : ((hi << (32u - s)) | (lo >> s));
}
uint SadU32(uint a, uint b, uint c) {
  return (a > b ? (a - b) : (b - a)) + c;
}
// Opérations f32 dans des fonctions `precise` (dxc ne replie plus x + 0, x - x ni 0 × x). Le pilote AMD
// (RX 9070 XT) replie pourtant fadd x, 0.0 et fsub x, x, et Intel fsub 0.0, x : le signe d'un résultat
// exactement nul et x - x sont donc fixés en entier.
uint F32ShiftJam(uint value, uint distance) {
  if (distance == 0u) return value;
  if (distance >= 32u) return value != 0u ? 1u : 0u;
  return (value >> distance) | ((value << (32u - distance)) != 0u ? 1u : 0u);
}
uint F32Round(uint sign, int exponent, uint sig) {
  // Biased exponent, hidden bit at 26, three guard/round/sticky bits.
  if (exponent < 1) { sig = F32ShiftJam(sig, uint(1 - exponent)); exponent = 1; }
  uint rounded = (sig + 3u + ((sig >> 3) & 1u)) >> 3;
  if (rounded >= 0x1000000u) { rounded >>= 1; ++exponent; }
  if (exponent >= 255) return sign | 0x7F800000u;
  if (rounded < 0x800000u) return sign | rounded;
  return sign | (uint(exponent) << 23) | (rounded & 0x7FFFFFu);
}
uint F32Significand(uint magnitude) {
  return (magnitude & 0x7FFFFFu) | (magnitude >= 0x800000u ? 0x800000u : 0u);
}
uint F32Add(uint a, uint b) {
  if (IsNanF32(a)) return a | 0x400000u;
  if (IsNanF32(b)) return b | 0x400000u;
  uint x = a & 0x7FFFFFFFu, y = b & 0x7FFFFFFFu;
  if (x == 0x7F800000u) return y == x && a != b ? 0xFFC00000u : a;
  if (y == 0x7F800000u) return b;
  if (x == y && (x == 0u || a != b)) return a & b & 0x80000000u;
  uint large = max(x, y), small = min(x, y), sign = (x >= y ? a : b) & 0x80000000u;
  int e = int(max(1u, large >> 23));
  uint u = F32Significand(large) << 3;
  uint v = F32ShiftJam(F32Significand(small) << 3, uint(e) - max(1u, small >> 23));
  uint r;
  if (((a ^ b) >> 31) == 0u) {
    r = u + v;
    if (r >= 0x8000000u) { r = F32ShiftJam(r, 1u); ++e; }
  } else {
    r = u - v;
    uint shift = min(uint(e - 1), 26u - firstbithigh(r));
    r <<= shift;
    e -= int(shift);
  }
  return F32Round(sign, e, r);
}
uint F32Sub(uint a, uint b) {
  if (IsNanF32(a)) return a | 0x400000u;
  if (IsNanF32(b)) return b | 0x400000u;
  return F32Add(a, b ^ 0x80000000u);
}
uint F32Mul(uint a, uint b) {
  if (IsNanF32(a)) return a | 0x400000u;
  if (IsNanF32(b)) return b | 0x400000u;
  uint x = a & 0x7FFFFFFFu, y = b & 0x7FFFFFFFu, sign = (a ^ b) & 0x80000000u;
  if (x == 0x7F800000u || y == 0x7F800000u) return min(x, y) == 0u ? 0xFFC00000u : sign | 0x7F800000u;
  if (min(x, y) == 0u) return sign;
  uint u = F32Significand(x), v = F32Significand(y);
  uint i = 23u - firstbithigh(u), j = 23u - firstbithigh(v);
  u <<= i; v <<= j;
  uint hi = MulHiU32(u, v), lo = u * v;
  uint extra = hi >> 15, shift = 20u + extra;
  int e = int(max(1u, x >> 23) + max(1u, y >> 23) + extra) - 127 - int(i + j);
  uint sig = (hi << (32u - shift)) | (lo >> shift) | ((lo << (32u - shift)) != 0u ? 1u : 0u);
  return F32Round(sign, e, sig);
}
uint F32MinMax(uint a, uint b, bool maximum) {
  if (IsNanF32(a)) return a | 0x400000u;
  if (IsNanF32(b) || ((a | b) & 0x7FFFFFFFu) == 0u) return a;
  uint ka = a ^ ((a >> 31) != 0u ? 0xFFFFFFFFu : 0x80000000u);
  uint kb = b ^ ((b >> 31) != 0u ? 0xFFFFFFFFu : 0x80000000u);
  return (maximum ? kb > ka : kb < ka) ? b : a;
}
uint MulLegacyF32(uint a, uint b) {
  float x = asfloat(a), y = asfloat(b);
  return (x == 0.0f || y == 0.0f) ? 0u : asuint(x * y);
}
uint RndneF32(uint a) {
  // Arrondi au pair en entier : le pilote NVIDIA 621.xx (RTX 4060, RTX 5070 Ti, #227) replie
  // Round_ne(2.5) en 3.0 après une barrière ; en entier le résultat suit IEEE 754 (signe de ±0, NaN).
  // Une seule sortie : avec des sorties anticipées, le pilote AMD de la RX 9070 XT compile mal un
  // v_bfrev_b32 voisin (bit 30 en trop, #160). Les cas sont calculés puis choisis à la fin.
  uint sign_bit = a & 0x80000000u, mag = a & 0x7FFFFFFFu;
  uint shift = (mag >= 0x3F800000u && mag < 0x4B000000u) ? 150u - (mag >> 23) : 1u;
  uint mask = (1u << shift) - 1u, half_bit = 1u << (shift - 1u);
  uint rem = mag & mask, rounded = mag & ~mask;
  bool up = rem > half_bit || (rem == half_bit && (rounded & (1u << shift)) != 0u);
  uint mid = sign_bit | (rounded + (up ? (1u << shift) : 0u));
  uint big = mag > 0x7F800000u ? (a | 0x400000u) : a;
  uint small = mag < 0x3F800000u ? (mag <= 0x3F000000u ? sign_bit : (sign_bit | 0x3F800000u)) : mid;
  return mag >= 0x4B000000u ? big : small;
}
uint PermB32(uint a, uint b, uint c) {
  uint r = 0u;
  [unroll] for (uint i = 0u; i < 4u; ++i) {
    uint sel = (c >> (i * 8u)) & 0xFFu;
    uint bt = 0xFFu;
    if (sel < 4u) bt = (b >> (sel * 8u)) & 0xFFu;
    else if (sel < 8u) bt = (a >> ((sel - 4u) * 8u)) & 0xFFu;
    else if (sel == 8u) bt = (b & 0x00008000u) != 0u ? 0xFFu : 0u;
    else if (sel == 9u) bt = (b & 0x80000000u) != 0u ? 0xFFu : 0u;
    else if (sel == 10u) bt = (a & 0x00008000u) != 0u ? 0xFFu : 0u;
    else if (sel == 11u) bt = (a & 0x80000000u) != 0u ? 0xFFu : 0u;
    else if (sel == 12u) bt = 0u;
    r |= bt << (i * 8u);
  }
  return r;
}
""" + rdna2_hlsl_fma.HELPERS                  # FmaF32, FmaLegacyF32 (après MulHiU32)
