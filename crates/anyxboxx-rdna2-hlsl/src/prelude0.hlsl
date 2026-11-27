uint MulHiU32(uint a, uint b) {
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
// (y:x) >> d sur 64 bits (x : mot faible), les bits perdus posés dans le bit 0 (bit collant).
uint2 ShiftRightJam64(uint2 v, uint d) {
  if (d == 0u) return v;
  if (d < 32u) return uint2((v.x >> d) | (v.y << (32u - d)) | ((v.x << (32u - d)) != 0u ? 1u : 0u), v.y >> d);
  if (d < 64u) {
    uint lost = v.x | (d > 32u ? (v.y << (64u - d)) : 0u);
    return uint2((v.y >> (d - 32u)) | (lost != 0u ? 1u : 0u), 0u);
  }
  return uint2((v.x | v.y) != 0u ? 1u : 0u, 0u);
}
uint2 ShiftLeft64(uint2 v, uint s) {
  if (s == 0u) return v;
  if (s < 32u) return uint2(v.x << s, (v.y << s) | (v.x >> (32u - s)));
  return uint2(0u, v.x << (s - 32u));
}
uint LeadingZeros64(uint2 v) {
  return v.y != 0u ? 31u - firstbithigh(v.y) : 63u - firstbithigh(v.x);
}
// sig : bit 30 = 1 (si normal), 7 bits sous le dernier bit gardé ; valeur = sig × 2^(e - 156).
uint FmaRoundPack(uint sign, int e, uint sig) {
  if (e < 0) {
    uint d = uint(-e);
    sig = d < 31u ? ((sig >> d) | ((sig << (32u - d)) != 0u ? 1u : 0u)) : (sig != 0u ? 1u : 0u);
    e = 0;
  } else if (e > 0xFD || (e == 0xFD && sig >= 0x7FFFFFC0u)) {
    return (sign << 31) | 0x7F800000u;
  }
  uint rest = sig & 0x7Fu;
  sig = (sig + 0x40u) >> 7;
  if (rest == 0x40u) sig &= ~1u;
  if (sig == 0u) e = 0;
  return (sign << 31) + (uint(e) << 23) + sig;
}
uint FmaF32(uint a, uint b, uint c) {
  uint ea = (a >> 23) & 0xFFu, eb = (b >> 23) & 0xFFu, ec = (c >> 23) & 0xFFu;
  uint ma = a & 0x7FFFFFu, mb = b & 0x7FFFFFu, mc = c & 0x7FFFFFu;
  uint sp = (a ^ b) >> 31, sc = c >> 31;
  if (ea == 0xFFu && ma != 0u) return a | 0x400000u;
  if (eb == 0xFFu && mb != 0u) return b | 0x400000u;
  if (ec == 0xFFu && mc != 0u) return c | 0x400000u;
  bool za = (a & 0x7FFFFFFFu) == 0u, zb = (b & 0x7FFFFFFFu) == 0u, zc = (c & 0x7FFFFFFFu) == 0u;
  if (ea == 0xFFu || eb == 0xFFu) {
    if (za || zb || (ec == 0xFFu && sc != sp)) return 0x7FC00000u;
    return (sp << 31) | 0x7F800000u;
  }
  if (ec == 0xFFu) return c;
  if (za || zb) return zc ? ((sp & sc) << 31) : c;
  // Valeur = Z × 2^F ; bit implicite posé, exposant 1 pour les sous-normaux.
  ma |= ea != 0u ? 0x800000u : 0u;
  mb |= eb != 0u ? 0x800000u : 0u;
  mc |= ec != 0u ? 0x800000u : 0u;
  uint2 p = uint2(ma * mb, MulHiU32(ma, mb));
  uint sp61 = LeadingZeros64(p) - 2u;
  p = ShiftLeft64(p, sp61);
  int fp = int(max(ea, 1u) + max(eb, 1u)) - 300 - int(sp61);
  uint sc61 = zc ? 0u : 29u - firstbithigh(mc);
  uint2 q = uint2(0u, zc ? 0u : mc << sc61);
  int fq = zc ? fp : int(max(ec, 1u)) - 182 - int(sc61);
  bool qBig = fq > fp || (fq == fp && (q.y > p.y || (q.y == p.y && q.x > p.x)));
  uint2 x = qBig ? q : p, y = qBig ? p : q;
  int fx = max(fp, fq);
  uint sz = qBig ? sc : sp;
  y = ShiftRightJam64(y, uint(abs(fp - fq)));
  uint2 z = sp == sc ? uint2(x.x + y.x, x.y + y.y + (x.x + y.x < x.x ? 1u : 0u))
                     : uint2(x.x - y.x, x.y - y.y - (x.x < y.x ? 1u : 0u));
  if ((z.x | z.y) == 0u) return 0u;
  uint lz = LeadingZeros64(z);
  z = ShiftLeft64(z, lz - 1u);
  return FmaRoundPack(sz, fx - int(lz) + 189, z.y | (z.x != 0u ? 1u : 0u));
}
uint FmaLegacyF32(uint a, uint b, uint c) {
  // DX9 : 0 × n'importe quoi = 0, le résultat est alors c (rendu silencieux s'il est NaN).
  if ((a & 0x7FFFFFFFu) == 0u || (b & 0x7FFFFFFFu) == 0u) {
    return ((c >> 23) & 0xFFu) == 0xFFu && (c & 0x7FFFFFu) != 0u ? c | 0x400000u : c;
  }
  return FmaF32(a, b, c);
}

bool IsNanF64(uint lo, uint hi) {
  uint h = hi & 0x7FFFFFFFu;
  return h > 0x7FF00000u || (h == 0x7FF00000u && lo != 0u);
}
uint SatF64ToI32(uint lo, uint hi) {
  double x = asdouble(lo, hi);
  if (IsNanF64(lo, hi)) return 0u;
  if (x >= 2147483647.0) return 0x7FFFFFFFu;
  if (x <= -2147483648.0) return 0x80000000u;
  return asuint((int)x);
}
uint SatF64ToU32(uint lo, uint hi) {
  double x = asdouble(lo, hi);
  if (IsNanF64(lo, hi) || x <= 0.0) return 0u;
  return x >= 4294967295.0 ? 0xFFFFFFFFu : (uint)x;
}

double F32ToF64(uint a) {
  // Conversion exacte sur les bits (le GPU peut vider un dénormal f32 converti par (double)).
  uint s = a & 0x80000000u, e = (a >> 23) & 0xFFu, m = a & 0x7FFFFFu;
  if (e == 0u) return m == 0u ? asdouble(0u, s) : (s != 0u ? -1.0 : 1.0) * (double)m * asdouble(0u, 0x36A00000u);
  return asdouble(m << 29, s | ((e + 896u) << 20) | (m >> 3));
}
uint F64ToF32(double x) {
