
// float32 -> float16, subnormals kept (half_from_float): round to nearest even with overflow to infinity,
// or toward zero with finite values past the range giving 65504.
uint BufHalfFromFloat(uint f, bool rtz) {
  uint sign = (f >> 16) & 0x8000u, e = (f >> 23) & 0xFFu, m = f & 0x7FFFFFu;
  if (e == 0xFFu) {
    uint payload = m >> 13;
    return sign | 0x7C00u | (m != 0u && payload == 0u ? 1u : payload);
  }
  if (e < 101u) return sign;
  uint full = m | 0x800000u;
  bool normal = e > 112u;
  uint drop = normal ? 13u : 126u - e;
  uint q = full >> drop, r = full & ((1u << drop) - 1u), half = 1u << (drop - 1u);
  if (!rtz && (r > half || (r == half && (q & 1u) != 0u))) q += 1u;
  uint bits = normal ? ((e - 113u) << 10) + q : q;
  return sign | min(bits, rtz ? 31743u : 0x7C00u);
}

uint BufNarrow(uint info, uint value) {
  uint kind = info & 7u;
  if (kind == 5u || kind == 6u) return value & 0xFFFFu;
  return BufHalfFromFloat(value, kind == 7u);
}

uint BufWiden(uint info, uint c, uint h) {
  uint kind = info & 7u;
  if (kind == 6u) return BufSigned(h, 16u);
  if (kind == 7u && BufWidth(info, c) != 16u) return BufHalf(h);
  return h;
}

void BufWrite(uint address, uint size, uint4 e) {
  uint base = address & ~3u, a = address & 3u, shift = a * 8u, need = (a + size + 3u) >> 2;
  uint data[5] = { e.x, e.y, e.z, e.w, 0u };
  uint mask[5] = { 0u, 0u, 0u, 0u, 0u };
  for (uint j = 0u; j < 4u; j++)
    mask[j] = size >= 4u * j + 4u ? 0xFFFFFFFFu : (size > 4u * j ? (1u << (8u * (size - 4u * j))) - 1u : 0u);
  for (uint k = 0u; k < need; k++) {
    uint value = data[k] << shift, bits = mask[k] << shift;
    if (shift != 0u && k > 0u) {
      value |= data[k - 1u] >> (32u - shift);
      bits |= mask[k - 1u] >> (32u - shift);
    }
    if (bits == 0xFFFFFFFFu) g_memory.Store(base + 4u * k, value);
    else if (bits != 0u) {
      g_memory.InterlockedAnd(base + 4u * k, ~bits);
      g_memory.InterlockedOr(base + 4u * k, value & bits);
    }
  }
}

uint DotPacked(uint a, uint b, uint c, uint width, bool sgn) {
  // v_dot2/4/8 : c + somme des produits des éléments de `width` bits, modulo 2^32 (calcul non signé).
  uint r = c;
  [unroll] for (uint i = 0u; i < 32u; i += width) {
    uint x = (a >> i) & ((1u << width) - 1u), y = (b >> i) & ((1u << width) - 1u);
    if (sgn) {
      x = asuint(asint(a << (32u - width - i)) >> (32u - width));
      y = asuint(asint(b << (32u - width - i)) >> (32u - width));
    }
    r += x * y;
  }
  return r;
}

uint MsadU8(uint a, uint b) {
  // v_msad_u8 : octets où la référence b vaut 0 exclus de la somme.
  uint r = 0u;
  // [loop] et non [unroll] : déroulée, cette boucle est mal compilée par le pilote AMD RDNA 2 (#151).
  [loop] for (uint i = 0u; i < 32u; i += 8u) {
    uint x = (a >> i) & 0xFFu, y = (b >> i) & 0xFFu;
    if (y != 0u) r += x > y ? x - y : y - x;
  }
  return r;
}
uint SadWindow(uint lo, uint hi, uint k, uint b, bool masked) {
  uint w = AlignBit(hi, lo, 8u * k);              // S0[8k+31:8k]
  return masked ? MsadU8(w, b) : SadU8(w, b);
}
uint2 QsadPk(uint lo, uint hi, uint b, uint2 c, bool masked) {
  uint r0 = (SadWindow(lo, hi, 0u, b, masked) + (c.x & 0xFFFFu)) & 0xFFFFu;
  uint r1 = (SadWindow(lo, hi, 1u, b, masked) + (c.x >> 16)) & 0xFFFFu;
  uint r2 = (SadWindow(lo, hi, 2u, b, masked) + (c.y & 0xFFFFu)) & 0xFFFFu;
  uint r3 = (SadWindow(lo, hi, 3u, b, masked) + (c.y >> 16)) & 0xFFFFu;
  return uint2(r0 | (r1 << 16), r2 | (r3 << 16));
}
uint4 MqsadU32(uint lo, uint hi, uint b, uint4 c) {
  return uint4(SadWindow(lo, hi, 0u, b, true) + c.x, SadWindow(lo, hi, 1u, b, true) + c.y,
               SadWindow(lo, hi, 2u, b, true) + c.z, SadWindow(lo, hi, 3u, b, true) + c.w);
}

uint F16ToF32(uint h) {
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

uint ClassF16(uint h) {
  uint e = (h >> 10) & 0x1Fu, m = h & 0x3FFu, n = (h >> 15) & 1u;
  if (e == 0x1Fu) return m != 0u ? ((m >> 9) & 1u) : (n != 0u ? 2u : 9u);
  if (e == 0u) return m == 0u ? (n != 0u ? 5u : 6u) : (n != 0u ? 4u : 7u);
  return n != 0u ? 3u : 8u;
}
int Sext16(uint x) {
  // Low 16 bits of x, sign-extended (bits 31:16 are ignored).
  return asint(x << 16) >> 16;
}
uint SatI16(int x) { return asuint(clamp(x, -32768, 32767)); }
uint AddSatI16(uint a, uint b, bool subtract) {
  // Signed 16-bit a + b (or a - b) saturated, from the sign bits in uint. Written as
  // SatI16(Sext16(a) + Sext16(b)) then masked to 16 bits, the sign-extended high half reached the
  // destination on an NVIDIA RTX 4060 Laptop GPU (WARP and Intel UHD were right).
  uint r = (subtract ? a - b : a + b) & 0xFFFFu;
  uint o = subtract ? (a ^ b) & (a ^ r) : (a ^ r) & (b ^ r);
  return (o & 0x8000u) != 0u ? ((a & 0x8000u) != 0u ? 0x8000u : 0x7FFFu) : r;
}
uint AddSatI32(uint a, uint b, bool sat) {
  // a + b, or with `sat` the signed sum saturated to [0x80000000, 0x7FFFFFFF].
  uint r = a + b;
  bool overflow = ((a ^ r) & (b ^ r) & 0x80000000u) != 0u;
  return sat && overflow ? ((a & 0x80000000u) != 0u ? 0x80000000u : 0x7FFFFFFFu) : r;
}
uint SubSatI32(uint a, uint b, bool sat) {
  uint r = a - b;
  bool overflow = ((a ^ b) & (a ^ r) & 0x80000000u) != 0u;
  return sat && overflow ? ((a & 0x80000000u) != 0u ? 0x80000000u : 0x7FFFFFFFu) : r;
}

uint F32Clamp01(uint f) {
  if ((f & 0x7FFFFFFFu) > 0x7F800000u || (f & 0x80000000u) != 0u) return 0u;
  return min(f, 0x3F800000u);
}


bool GatherIsNan(uint bits) { return (bits & 0x7F800000u) == 0x7F800000u && (bits & 0x007FFFFFu) != 0u; }

int GatherKey(uint bits) {
  uint magnitude = (bits & 0x7F800000u) == 0u ? 0u : (bits & 0x7FFFFFFFu);
  return (bits & 0x80000000u) != 0u ? -int(magnitude) : int(magnitude);
}

uint GatherTest(uint z_bits, uint t_bits, uint func) {
  if (func == 0u) return 0u;
  if (func == 7u) return 0x3f800000u;
  bool p = false;
  if (GatherIsNan(z_bits) || GatherIsNan(t_bits)) {
    p = func == 5u;
  } else {
    int a = GatherKey(z_bits);
    int b = GatherKey(t_bits);
    if (func == 1u) p = a < b;
    else if (func == 2u) p = a == b;
    else if (func == 3u) p = a <= b;
    else if (func == 4u) p = a > b;
    else if (func == 5u) p = a != b;
    else p = a >= b;
  }
  return p ? 0x3f800000u : 0u;
}

uint4 GatherCompare(uint4 t, uint4 q, uint z_bits, uint s_bits, uint t_bits, uint unorm_bit, int2 off) {
  uint un = unorm_bit | ((q.x >> 15) & 1u);
  uint cx = q.x & 7u;
  uint cy = (q.x >> 3) & 7u;
  uint func = (q.x >> 12) & 7u;
  uint border = (q.w >> 30) & 3u;
  uint w = ImageWidth(t);
  uint h = ImageHeight(t);
  float fx = 0.0f;
  float fy = 0.0f;
  int2 xb = ImageBilinearAxis(s_bits, w, un, off.x, fx);
  int2 yb = ImageBilinearAxis(t_bits, h, un, off.y, fy);
  int x0 = ImageClamp(xb.x, w, cx);
  int x1 = ImageClamp(xb.y, w, cx);
  int y0 = ImageClamp(yb.x, h, cy);
  int y1 = ImageClamp(yb.y, h, cy);
  return uint4(GatherTest(z_bits, ImageFetch(t, x0, y1, border).x, func),
               GatherTest(z_bits, ImageFetch(t, x1, y1, border).x, func),
               GatherTest(z_bits, ImageFetch(t, x1, y0, border).x, func),
               GatherTest(z_bits, ImageFetch(t, x0, y0, border).x, func));
}

