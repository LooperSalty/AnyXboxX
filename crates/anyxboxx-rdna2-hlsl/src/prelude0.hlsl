// Généré par AnyXboxX (emu/rdna2_hlsl.py) : programme RDNA 2 recompilé en HLSL.
#define END_PC 0xFFFFFFFFu
RWByteAddressBuffer g_memory : register(u0);
groupshared uint g_lds[8128];      // D3D12 : 32 Kio de mémoire partagée par groupe au plus
groupshared uint g_ballot;
groupshared uint g_lanes[32];
groupshared uint g_owner[32];

uint Ballot(uint lane, bool bit) {
  GroupMemoryBarrierWithGroupSync();
  if (lane == 0) g_ballot = 0;
  GroupMemoryBarrierWithGroupSync();
  if (bit) InterlockedOr(g_ballot, 1u << lane);
  GroupMemoryBarrierWithGroupSync();
  return g_ballot;
}

uint ReadLane(uint lane, uint value, uint other) {
  GroupMemoryBarrierWithGroupSync();
  g_lanes[lane] = value;
  GroupMemoryBarrierWithGroupSync();
  return g_lanes[other & 31u];
}

uint ForwardLane(uint lane, bool active, uint value, uint target) {
  // ds_permute_b32 : la ligne active de plus grand numéro qui vise une ligne lui donne sa valeur.
  target &= 31u;
  GroupMemoryBarrierWithGroupSync();
  g_owner[lane] = 0u;
  GroupMemoryBarrierWithGroupSync();
  if (active) InterlockedMax(g_owner[target], lane + 1u);
  GroupMemoryBarrierWithGroupSync();
  if (active && g_owner[target] == lane + 1u) g_lanes[target] = value;
  GroupMemoryBarrierWithGroupSync();
  return g_owner[lane] != 0u ? g_lanes[lane] : 0u;
}

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
  uint field = (v >> (off & 31u)) & ((1u << w) - 1u);
  uint shift = 32u - w;
  return asuint((asint(field) << shift) >> shift);
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
uint MulLegacyF32(uint a, uint b) {
  float x = asfloat(a), y = asfloat(b);
  return (x == 0.0f || y == 0.0f) ? 0u : asuint(x * y);
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
  // Arrondi au plus près pair d'un double vers f32, dénormaux et dépassement compris.
  uint lo, hi; asuint(x, lo, hi);
  uint s = hi & 0x80000000u, ef = (hi >> 20) & 0x7FFu;
  if (ef == 0u) return s;
  int e = int(ef) - 1023;
  uint sig = 0x80000000u | ((hi & 0xFFFFFu) << 11) | (lo >> 21);
  bool sticky = (lo & 0x1FFFFFu) != 0u;
  uint shift = e < -126 ? uint(8 + (-126 - e)) : 8u;
  if (shift > 32u) return s;
  uint kept = shift == 32u ? 0u : sig >> shift;
  uint rest = shift == 32u ? sig : sig & ((1u << shift) - 1u);
  uint half = 1u << (shift - 1u);
  if (rest > half || (rest == half && (sticky || (kept & 1u) != 0u))) kept += 1u;
  if (e < -126) return s | kept;
  if (kept == 0x1000000u) { kept = 0x800000u; e += 1; }
  if (e > 127) return s | 0x7F800000u;
  return s | (uint(e + 127) << 23) | (kept & 0x7FFFFFu);
}
bool EqF32(uint a, uint b) {
  return !IsNanF32(a) && !IsNanF32(b) && (a == b || ((a | b) & 0x7FFFFFFFu) == 0u);
}
uint ScaleF32(uint a) {
  // ldexp(a, 64) ; infini et NaN inchangés.
  return ((a >> 23) & 0xFFu) == 0xFFu ? a : F64ToF32(F32ToF64(a) * asdouble(0u, 0x43F00000u));
}
bool QuotientIsDenormalF32(uint n, uint d) {
  // |n / d| arrondi est un dénormal non nul : 2^-150 < |n / d| < (2^24 - 1) × 2^-150 (produits exacts).
  precise double x = F32ToF64(n & 0x7FFFFFFFu), y = F32ToF64(d & 0x7FFFFFFFu);
  precise double low = y * asdouble(0u, 0x36900000u), high = y * ((double)0xFFFFFFu * asdouble(0u, 0x36900000u));
  return x > low && x < high;
