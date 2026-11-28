uint WaveBallot32(bool bit) {
  uint4 b = WaveActiveBallot(bit && !IsHelperLane());
  uint h = WaveGetLaneIndex() >> 5;
  return h == 0u ? b.x : (h == 1u ? b.y : (h == 2u ? b.z : b.w));
}
uint Ballot(uint lane, bool bit) { return WaveBallot32(bit); }
uint ReadLane(uint lane, uint value, uint other) {
  uint base = WaveGetLaneIndex() & ~31u;
  return WaveReadLaneAt(value, base + (other & 31u));
}
