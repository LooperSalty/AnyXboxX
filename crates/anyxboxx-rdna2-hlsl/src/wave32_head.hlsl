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

