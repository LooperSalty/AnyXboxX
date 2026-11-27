// Généré par AnyXboxX (emu/rdna2_hlsl_wave64.py) : programme RDNA 2 wave64 recompilé en HLSL.
// Wave virtuel de 64 lignes = un groupe de 64 threads (ligne n = SV_GroupIndex n), quelle que soit la
// taille des waves du GPU : masques et lectures de ligne passent par la mémoire groupshared.
#define END_PC 0xFFFFFFFFu
#define LANE_ON Bit64(s[126], s[127], lane)
RWByteAddressBuffer g_memory : register(u0);
groupshared uint g_lds[8128];      // D3D12 : 32 Kio de mémoire partagée par groupe au plus
// Un seul tableau d'échange pour Ballot64 (mots 0 et 1) et ReadLane64 (64 mots) : 8128 + 64 = 8192 mots.
// Chaque appel commence par une barrière, donc les lectures de l'appel précédent sont finies.
groupshared uint g_exchange[64];

bool Bit64(uint low, uint high, uint lane) {
  return (((lane < 32u ? low : high) >> (lane & 31u)) & 1u) != 0u;
}

uint2 Ballot64(uint lane, bool bit) {
  GroupMemoryBarrierWithGroupSync();
  if (lane < 2u) g_exchange[lane] = 0u;
  GroupMemoryBarrierWithGroupSync();
  if (bit) InterlockedOr(g_exchange[lane >> 5], 1u << (lane & 31u));
  GroupMemoryBarrierWithGroupSync();
  return uint2(g_exchange[0], g_exchange[1]);
}

uint ReadLane64(uint lane, uint value, uint other) {
  GroupMemoryBarrierWithGroupSync();
  g_exchange[lane] = value;
  GroupMemoryBarrierWithGroupSync();
  return g_exchange[other & 63u];
}

uint FirstLane64(uint low, uint high) {     // plus basse ligne active, 0 si EXEC est nul
  return low != 0u ? firstbitlow(low) : (high != 0u ? 32u + firstbitlow(high) : 0u);
}

