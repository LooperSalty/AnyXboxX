uint F16Magnitude(uint a) {   // integer part of a finite |x| >= 1 (a = bits without the sign)
  uint e = a >> 10, m = (a & 0x3FFu) | 0x400u;
  return e >= 25u ? m << (e - 25u) : m >> (25u - e);
}
uint F16ToU16(uint h) {
  uint a = h & 0x7FFFu;
  if (a > 0x7C00u || (h & 0x8000u) != 0u || a < 0x3C00u) return 0u;
  return a == 0x7C00u ? 0xFFFFu : F16Magnitude(a);
}
uint F16ToI16(uint h) {
  uint a = h & 0x7FFFu;
  if (a > 0x7C00u || a < 0x3C00u) return 0u;
  uint m = a >= 0x7800u ? 0x8000u : F16Magnitude(a);         // 0x7800 = 32768, infinity included
  if ((h & 0x8000u) != 0u) return (0u - m) & 0xFFFFu;
  return m > 0x7FFFu ? 0x7FFFu : m;
}
uint RoundShift(uint p, uint shift) {         // p / 2^shift rounded to nearest even, 1 <= shift <= 31
  uint q = p >> shift, rest = p & ((1u << shift) - 1u), half = 1u << (shift - 1u);
  return q + ((rest > half || (rest == half && (q & 1u) != 0u)) ? 1u : 0u);
}
uint F16Norm(uint h, uint scale, bool isSigned) {   // round_even(x limited to [-1 or 0, 1] * scale), NaN -> 0
  uint a = h & 0x7FFFu, e = a >> 10;
  bool negative = (h & 0x8000u) != 0u;
  if (a > 0x7C00u || (negative && !isSigned)) return 0u;
  uint q = scale;
  if (a < 0x3C00u) {
    uint m = e != 0u ? (a & 0x3FFu) | 0x400u : a;
    q = RoundShift(m * scale, 25u - max(e, 1u));                // m * scale < 2^27, shift 11..24
  }
  return negative ? (0u - q) & 0xFFFFu : q;
}
uint F32Norm16(uint f, uint scale, bool isSigned) { // the same from an f32, on a 40-bit product in two words
  uint a = f & 0x7FFFFFFFu, e = a >> 23;
  bool negative = (f >> 31) != 0u;
  if (a > 0x7F800000u || (negative && !isSigned)) return 0u;
  uint q = scale;
  if (a < 0x3F800000u) {
    uint shift = 150u - e;                    // |x| = m * 2^-shift, shift >= 24
    if (shift > 40u) {
      q = 0u;                                 // the product is below 2^40 <= half of 2^shift
    } else {
      uint m = (a & 0x7FFFFFu) | 0x800000u;
      uint low = (m & 0xFFFu) * scale, high = (m >> 12) * scale;  // m * scale = high * 2^12 + low
      uint lo = low + (high << 12);
      uint hi = (high >> 20) + (lo < low ? 1u : 0u);
      uint b = shift - 1u;                    // weight of the half bit
      uint quotient = shift >= 32u ? hi >> (shift - 32u) : (lo >> shift) | (hi << (32u - shift));
      bool halfbit = ((b >= 32u ? hi >> (b - 32u) : lo >> b) & 1u) != 0u;
      bool sticky = b >= 32u ? (lo != 0u || (hi & ((1u << (b - 32u)) - 1u)) != 0u) : (lo & ((1u << b) - 1u)) != 0u;
      q = quotient + ((halfbit && (sticky || (quotient & 1u) != 0u)) ? 1u : 0u);
    }
  }
  return negative ? (0u - q) & 0xFFFFu : q;
}
uint F16Med3(uint a, uint b, uint c) {       // ISA pseudocode; F16Min/F16Max: emu/rdna2_hlsl_f16.py
  if (F16IsNan(a) || F16IsNan(b) || F16IsNan(c)) return F16Min(F16Min(a, b), c);
  uint m = F16Max(F16Max(a, b), c);
  if (m == a || ((m | a) & 0x7FFFu) == 0u) return F16Max(b, c);    // equal as floats: +0 == -0
  if (m == b || ((m | b) & 0x7FFFu) == 0u) return F16Max(a, c);
  return F16Max(a, b);
}

