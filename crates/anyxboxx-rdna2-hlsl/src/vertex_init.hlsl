struct VSOut { float4 position : SV_Position; float2 bary : TEXCOORD0; };
VSOut main(uint vertex : SV_VertexID) {
  uint lane = 0u;
  uint s[128];
  uint v[256];
  uint s_null = 0u;
  bool scc = false;
  for (uint i = 0; i < 128; ++i) s[i] = 0u;
  for (uint j = 0; j < 256; ++j) v[j] = 0u;
  s[126] = 1u;
  VSOut o_out;
  o_out.position = float4(0.0f, 0.0f, 0.0f, 1.0f);
  uint corner = vertex % 3u;     // i et j du rastériseur : (0,0), (1,0), (0,1)
  o_out.bary = float2(corner == 1u ? 1.0f : 0.0f, corner == 2u ? 1.0f : 0.0f);
  uint record = 327680u + vertex * 144u;
  v[0] = vertex;
