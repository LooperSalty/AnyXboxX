
void ImageStore(uint4 t, uint x, uint y, uint mip, uint dmask, uint4 vals) {
  uint w = ImageWidth(t);
  uint h = ImageHeight(t);
  if (mip != 0u || x >= w || y >= h || dmask == 0u) return;
  uint format = (t.y >> 20) & 0x1FFu;
  uint pixel = y * w + x;
  uint base = t.x << 8;
  if (format == 77u) {
    uint addr = base + pixel * 16u;
    if ((dmask & 1u) != 0u) g_memory.Store(addr, vals.x);
    if ((dmask & 2u) != 0u) g_memory.Store(addr + 4u, vals.y);
    if ((dmask & 4u) != 0u) g_memory.Store(addr + 8u, vals.z);
    if ((dmask & 8u) != 0u) g_memory.Store(addr + 12u, vals.w);
  } else if (format == 64u) {
    uint addr = base + pixel * 8u;
    if ((dmask & 1u) != 0u) g_memory.Store(addr, vals.x);
    if ((dmask & 2u) != 0u) g_memory.Store(addr + 4u, vals.y);
  } else if (format == 22u) {
    if ((dmask & 1u) != 0u) g_memory.Store(base + pixel * 4u, vals.x);
  } else if (format == 56u) {
    uint addr = base + pixel * 4u;
    uint word = (dmask == 0xFu) ? 0u : g_memory.Load(addr);
    if ((dmask & 1u) != 0u) word = (word & 0xFFFFFF00u) | ImageFloatToUnorm8(vals.x);
    if ((dmask & 2u) != 0u) word = (word & 0xFFFF00FFu) | (ImageFloatToUnorm8(vals.y) << 8);
    if ((dmask & 4u) != 0u) word = (word & 0xFF00FFFFu) | (ImageFloatToUnorm8(vals.z) << 16);
    if ((dmask & 8u) != 0u) word = (word & 0x00FFFFFFu) | (ImageFloatToUnorm8(vals.w) << 24);
    g_memory.Store(addr, word);
  } else if (format == 14u) {
    uint byte_addr = base + pixel * 2u;
    uint aligned = byte_addr & ~3u;
    uint shift = (byte_addr & 2u) * 8u;
    uint clear_mask = 0u;
    uint bits = 0u;
    if ((dmask & 1u) != 0u) { clear_mask |= 0xFFu << shift; bits |= ImageFloatToUnorm8(vals.x) << shift; }
    if ((dmask & 2u) != 0u) { clear_mask |= 0xFFu << (shift + 8u); bits |= ImageFloatToUnorm8(vals.y) << (shift + 8u); }
    if (clear_mask != 0u) {
      g_memory.InterlockedAnd(aligned, ~clear_mask);
      g_memory.InterlockedOr(aligned, bits);
    }
  } else if (format == 1u) {
    if ((dmask & 1u) != 0u) {
      uint byte_addr = base + pixel;
      uint aligned = byte_addr & ~3u;
      uint shift = (byte_addr & 3u) * 8u;
      g_memory.InterlockedAnd(aligned, ~(0xFFu << shift));
      g_memory.InterlockedOr(aligned, ImageFloatToUnorm8(vals.x) << shift);
    }
  }
}

uint4 ImageResInfo(uint4 t, uint mip) {
  return mip == 0u ? uint4(ImageWidth(t), ImageHeight(t), 1u, 1u) : uint4(0u, 0u, 0u, 1u);
}


static const uint BUF_FORMAT_INFO[128] = { 0x00000000u, 0x00007049u, 0x0000704Au, 0x0000704Bu, 0x0000704Cu, 0x0000784Du, 0x0000784Eu, 0x0000F089u, 0x0000F08Au, 0x0000F08Bu, 0x0000F08Cu, 0x0000F88Du, 0x0000F88Eu, 0x0000F08Fu, 0x000E7091u, 0x000E7092u, 0x000E7093u, 0x000E7094u, 0x000E7895u, 0x000E7896u, 0x0001F90Du, 0x0001F90Eu, 0x0001F90Fu, 0x001EF111u, 0x001EF112u, 0x001EF113u, 0x001EF114u, 0x001EF915u, 0x001EF916u, 0x001EF117u, 0x0254A119u, 0x0254A11Au, 0x0254A11Bu, 0x0254A11Cu, 0x0254A91Du, 0x0254A91Eu, 0x00000000u, 0x02949119u, 0x0294911Au, 0x0294911Bu, 0x0294911Cu, 0x0294991Du, 0x0294991Eu, 0x00000000u, 0x4A521121u, 0x4A521122u, 0x4A521123u, 0x4A521124u, 0x4A521925u, 0x4A521926u, 0x0A529121u, 0x0A529122u, 0x0A529123u, 0x0A529124u, 0x0A529925u, 0x0A529926u, 0x39CE7121u, 0x39CE7122u, 0x39CE7123u, 0x39CE7124u, 0x39CE7925u, 0x39CE7926u, 0x003FFA15u, 0x003FFA16u, 0x003FFA17u, 0x7BDEF221u, 0x7BDEF222u, 0x7BDEF223u, 0x7BDEF224u, 0x7BDEFA25u, 0x7BDEFA26u, 0x7BDEF227u, 0x07FFFB1Du, 0x07FFFB1Eu, 0x07FFFB1Fu, 0xFFFFFC25u, 0xFFFFFC26u, 0xFFFFFC27u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u, 0x00000000u };

uint BufFormatInfo(uint code, uint4 d) {
  if (((d.y >> 31) & 1u) != 0u || ((d.w >> 23) & 1u) != 0u) return 0u;
  return BUF_FORMAT_INFO[code & 0x7Fu];
}
uint BufWidth(uint info, uint c) { return ((info >> (12u + 5u * c)) & 31u) + 1u; }
uint BufCount(uint info) { return (info >> 3) & 7u; }
uint BufSize(uint info) { return (info >> 6) & 31u; }

// m / (2^n - 1) arrondi au plus proche, 1 <= n <= 16 : l'écriture binaire du quotient est m répété ;
// T (hi:lo, 64 bits) en garde les 64 premiers bits, la mantisse et le bit d'arrondi en sont tirés
// (jamais d'égalité : la suite répétée n'est jamais nulle). Méthode de norm_bits (Leonida), en mots de 32 bits.
uint BufUnorm(uint m, uint n) {
  if (m == 0u) return 0u;
  if (m == (1u << n) - 1u) return 0x3F800000u;
  uint hi = 0u, lo = 0u;
  for (int top = 64 - int(n); top > -int(n); top -= int(n)) {
    if (top >= 32) hi |= m << uint(top - 32);
    else if (top >= 0) {
      lo |= m << uint(top);
      if (uint(top) + n > 32u) hi |= m >> uint(32 - top);
    } else lo |= m >> uint(-top);
  }
  uint p = firstbithigh(hi);
  uint q = p >= 23u ? hi >> (p - 23u) : (hi << (23u - p)) | (lo >> (p + 9u));
  uint r = p >= 24u ? (hi >> (p - 24u)) & 1u : (lo >> (p + 8u)) & 1u;
  return ((p + 95u) << 23) + (q - 0x800000u) + r;
}

uint BufSigned(uint field, uint n) { return asuint(asint(field << (32u - n)) >> (32u - n)); }

uint BufSnorm(uint field, uint n) {
  int x = asint(BufSigned(field, n));
  int limit = int((1u << (n - 1u)) - 1u);
  if (x <= -limit) return 0xBF800000u;
  uint magnitude = BufUnorm(uint(abs(x)), n - 1u);
  return x < 0 ? magnitude | 0x80000000u : magnitude;
}

uint BufHalf(uint h) {
  uint sign = (h & 0x8000u) << 16, e = (h >> 10) & 31u, m = h & 0x3FFu;
  if (e == 31u) return sign | 0x7F800000u | (m << 13);
  if (e != 0u) return sign | ((e + 112u) << 23) | (m << 13);
  if (m == 0u) return sign;
  uint p = firstbithigh(m);
  return sign | ((p + 103u) << 23) | ((m << (23u - p)) & 0x7FFFFFu);
}

uint BufComponent(uint kind, uint field, uint n) {
  if (kind == 1u) return BufUnorm(field, n);
  if (kind == 2u) return BufSnorm(field, n);
  if (kind == 3u) return asuint(float(field));
  if (kind == 4u) return asuint(float(asint(BufSigned(field, n))));
  if (kind == 6u) return BufSigned(field, n);
  if (kind == 7u && n == 16u) return BufHalf(field);
  return field;
}

uint4 BufDecode(uint info, uint4 w) {
  uint4 c = uint4(0u, 0u, 0u, 0u);
  uint position = 0u;
  for (uint i = 0u; i < BufCount(info); i++) {
    uint n = BufWidth(info, i);
    uint word = w[position >> 5];
    // Champ par décalage à gauche puis à droite : avec (word >> p) & masque, le pilote AMD RDNA 2 perd le >> 16 des moitiés hautes.
    uint field = n >= 32u ? word : (word << (32u - n - (position & 31u))) >> (32u - n);
    c[i] = BufComponent(info & 7u, field, n);
    position += n;
  }
  return c;
}

uint4 BufRead(uint address, uint size) {
  uint base = address & ~3u, shift = (address & 3u) * 8u, need = ((address & 3u) + size + 3u) >> 2;
  uint raw[5] = { 0u, 0u, 0u, 0u, 0u };
  for (uint k = 0u; k < need; k++) raw[k] = g_memory.Load(base + 4u * k);
  uint4 w;
  for (uint j = 0u; j < 4u; j++) w[j] = shift == 0u ? raw[j] : (raw[j] >> shift) | (raw[j + 1u] << (32u - shift));
  return w;
}

uint BufSelect(uint4 c, uint info, uint sel) {
  if (info == 0u) return 0u;
  uint kind = info & 7u;
  if (sel == 1u) return kind == 5u || kind == 6u ? 1u : 0x3F800000u;
  if (sel >= 4u && sel - 4u < BufCount(info)) return c[sel - 4u];
  return 0u;
}

uint BufField(uint kind, uint value, uint n) {
  if (n >= 32u) return value;
  if (kind == 5u) return min(value, (1u << n) - 1u);
  int high = int((1u << (n - 1u)) - 1u);
  return uint(clamp(asint(value), -high - 1, high)) & ((1u << n) - 1u);
}

uint4 BufEncode(uint info, uint4 v) {
  uint4 w = uint4(0u, 0u, 0u, 0u);
  uint position = 0u;
  for (uint i = 0u; i < BufCount(info); i++) {
    uint n = BufWidth(info, i);
    w[position >> 5] |= BufField(info & 7u, v[i], n) << (position & 31u);
    position += n;
  }
  return w;
}

bool BufStoreSelectsOk(uint info, uint word3, uint count) {
  uint n = BufCount(info);
  if (((info >> 11) & 1u) == 0u || n != count) return false;
  for (uint i = 0u; i < n; i++) if (((word3 >> (3u * i)) & 7u) != 4u + i) return false;
  return n != 2u || ((word3 >> 6) & 7u) <= 1u;
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

[numthreads(32, 1, 1)]
void main(uint lane : SV_GroupIndex) {
  uint s[128];
  uint v[256];
  uint s_null = 0u;
  bool scc = false;
  // Dérouler l'initialisation scalaire évite un résultat incorrect sur WARP (#142).
  [unroll] for (uint i = 0; i < 128; ++i) s[i] = 0u;
  for (uint j = 0; j < 256; ++j) v[j] = 0u;
  v[0] = lane;
  s[126] = 0xFFFFFFFFu;
  // La mémoire groupshared n'est pas remise à zéro par D3D12 : on la vide, comme l'interpréteur.
  for (uint w = lane; w < 8128u; w += 32u) g_lds[w] = 0u;
  GroupMemoryBarrierWithGroupSync();
