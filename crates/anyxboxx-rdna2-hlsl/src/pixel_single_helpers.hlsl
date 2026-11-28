uint Ballot(uint lane, bool bit) { return bit ? 1u : 0u; }            // wave d'une ligne
uint ReadLane(uint lane, uint value, uint other) { return value; }
