// Synthetic gfx1030 programs for `skinning_bench`, written by hand and assembled with LLVM's
// `llvm-mc -arch=amdgcn -mcpu=gfx1030 -mattr=+wavefrontsize32`; each word's text is beside it
// (or the program's name gives the one instruction it tests). No title code.
pub(super) const SKIN_LOOP: &[u32] = &[
    0xBE860304, // s_mov_b32 s6, s4
    0xBE870380, // s_mov_b32 s7, 0
    0xF4080203, 0xFA000000, // s_load_dwordx4 s[8:11], s[6:7], 0x0
    0xF4080303, 0xFA000020, // s_load_dwordx4 s[12:15], s[6:7], 0x20
    0xF4080403, 0xFA000040, // s_load_dwordx4 s[16:19], s[6:7], 0x40
    0xF4080603, 0xFA000060, // s_load_dwordx4 s[24:27], s[6:7], 0x60
    0xD7460001, 0x04010A05, // v_lshl_add_u32 v1, s5, 5, v0
    0x34040285, // v_lshlrev_b32_e32 v2, 5, v1
    0xBF8CC07F, // s_waitcnt lgkmcnt(0)
    0xE0381000, 0x80020402, // buffer_load_dwordx4 v[4:7], v2, s[8:11], 0 offen
    0xE0381010, 0x80020802, // buffer_load_dwordx4 v[8:11], v2, s[8:11], 0 offen offset:16
    0x7E180280, // v_mov_b32_e32 v12, 0
    0x7E1A0280, // v_mov_b32_e32 v13, 0
    0x7E1C0280, // v_mov_b32_e32 v14, 0
    0x7E1E0280, // v_mov_b32_e32 v15, 0
    0x7E200280, // v_mov_b32_e32 v16, 0
    0x7E220280, // v_mov_b32_e32 v17, 0
    0x7E240280, // v_mov_b32_e32 v18, 0
    0x7E260280, // v_mov_b32_e32 v19, 0
    0x7E280280, // v_mov_b32_e32 v20, 0
    0x7E2A0280, // v_mov_b32_e32 v21, 0
    0x7E2C0280, // v_mov_b32_e32 v22, 0
    0x7E2E0280, // v_mov_b32_e32 v23, 0
    0x7E300280, // v_mov_b32_e32 v24, 0
    0x7E320280, // v_mov_b32_e32 v25, 0
    0x7E340280, // v_mov_b32_e32 v26, 0
    0x7E360280, // v_mov_b32_e32 v27, 0
    0x7E380280, // v_mov_b32_e32 v28, 0
    0x7E3A0280, // v_mov_b32_e32 v29, 0
    0x7E3C0280, // v_mov_b32_e32 v30, 0
    0x7E3E0280, // v_mov_b32_e32 v31, 0
    0x7E400280, // v_mov_b32_e32 v32, 0
    0x7E420280, // v_mov_b32_e32 v33, 0
    0x7E440280, // v_mov_b32_e32 v34, 0
    0x7E460280, // v_mov_b32_e32 v35, 0
    0xBF8C3F70, // s_waitcnt vmcnt(0)
    0xBE940380, // s_mov_b32 s20, 0
    0xBE950380, // s_mov_b32 s21, 0
    0xD5480024, 0x0220290A, // v_bfe_u32 v36, v10, s20, 8
    0xD5480025, 0x0220290B, // v_bfe_u32 v37, v11, s20, 8
    0x7E4A2325, // v_cvt_f32_ubyte0_e32 v37, v37
    0x104A4AFF, 0x3B808081, // v_mul_f32_e32 v37, 0x3b808081, v37
    0x164C48B0, // v_mul_u32_u24_e32 v38, 48, v36
    0xE0381000, 0x80032826, // buffer_load_dwordx4 v[40:43], v38, s[12:15], 0 offen
    0xE0381010, 0x80032C26, // buffer_load_dwordx4 v[44:47], v38, s[12:15], 0 offen offset:16
    0xE0381020, 0x80033026, // buffer_load_dwordx4 v[48:51], v38, s[12:15], 0 offen offset:32
    0xE0381000, 0x80043426, // buffer_load_dwordx4 v[52:55], v38, s[16:19], 0 offen
    0xE0381010, 0x80043826, // buffer_load_dwordx4 v[56:59], v38, s[16:19], 0 offen offset:16
    0xE0381020, 0x80043C26, // buffer_load_dwordx4 v[60:63], v38, s[16:19], 0 offen offset:32
    0xBF8C3F70, // s_waitcnt vmcnt(0)
    0x56185125, // v_fmac_f32_e32 v12, v37, v40
    0x561A5325, // v_fmac_f32_e32 v13, v37, v41
    0x561C5525, // v_fmac_f32_e32 v14, v37, v42
    0x561E5725, // v_fmac_f32_e32 v15, v37, v43
    0x56205925, // v_fmac_f32_e32 v16, v37, v44
    0x56225B25, // v_fmac_f32_e32 v17, v37, v45
    0x56245D25, // v_fmac_f32_e32 v18, v37, v46
    0x56265F25, // v_fmac_f32_e32 v19, v37, v47
    0x56286125, // v_fmac_f32_e32 v20, v37, v48
    0x562A6325, // v_fmac_f32_e32 v21, v37, v49
    0x562C6525, // v_fmac_f32_e32 v22, v37, v50
    0x562E6725, // v_fmac_f32_e32 v23, v37, v51
    0x56306925, // v_fmac_f32_e32 v24, v37, v52
    0x56326B25, // v_fmac_f32_e32 v25, v37, v53
    0x56346D25, // v_fmac_f32_e32 v26, v37, v54
    0x56366F25, // v_fmac_f32_e32 v27, v37, v55
    0x56387125, // v_fmac_f32_e32 v28, v37, v56
    0x563A7325, // v_fmac_f32_e32 v29, v37, v57
    0x563C7525, // v_fmac_f32_e32 v30, v37, v58
    0x563E7725, // v_fmac_f32_e32 v31, v37, v59
    0x56407925, // v_fmac_f32_e32 v32, v37, v60
    0x56427B25, // v_fmac_f32_e32 v33, v37, v61
    0x56447D25, // v_fmac_f32_e32 v34, v37, v62
    0x56467F25, // v_fmac_f32_e32 v35, v37, v63
    0x80148814, // s_add_u32 s20, s20, 8
    0x80158115, // s_add_u32 s21, s21, 1
    0xBF0A0015, // s_cmp_lt_u32 s21, s0
    0xBF85FFCF, // s_cbranch_scc1 LOOP
    0xD54B0040, 0x043E090C, // v_fma_f32 v64, v12, v4, v15
    0x56800B0D, // v_fmac_f32_e32 v64, v13, v5
    0x56800D0E, // v_fmac_f32_e32 v64, v14, v6
    0xD54B0041, 0x044E0910, // v_fma_f32 v65, v16, v4, v19
    0x56820B11, // v_fmac_f32_e32 v65, v17, v5
    0x56820D12, // v_fmac_f32_e32 v65, v18, v6
    0xD54B0042, 0x045E0914, // v_fma_f32 v66, v20, v4, v23
    0x56840B15, // v_fmac_f32_e32 v66, v21, v5
    0x56840D16, // v_fmac_f32_e32 v66, v22, v6
    0xD54B0048, 0x046E0918, // v_fma_f32 v72, v24, v4, v27
    0x56900B19, // v_fmac_f32_e32 v72, v25, v5
    0x56900D1A, // v_fmac_f32_e32 v72, v26, v6
    0xD54B0049, 0x047E091C, // v_fma_f32 v73, v28, v4, v31
    0x56920B1D, // v_fmac_f32_e32 v73, v29, v5
    0x56920D1E, // v_fmac_f32_e32 v73, v30, v6
    0xD54B004A, 0x048E0920, // v_fma_f32 v74, v32, v4, v35
    0x56940B21, // v_fmac_f32_e32 v74, v33, v5
    0x56940D22, // v_fmac_f32_e32 v74, v34, v6
    0x10880F0C, // v_mul_f32_e32 v68, v12, v7
    0x5688110D, // v_fmac_f32_e32 v68, v13, v8
    0x5688130E, // v_fmac_f32_e32 v68, v14, v9
    0x108A0F10, // v_mul_f32_e32 v69, v16, v7
    0x568A1111, // v_fmac_f32_e32 v69, v17, v8
    0x568A1312, // v_fmac_f32_e32 v69, v18, v9
    0x108C0F14, // v_mul_f32_e32 v70, v20, v7
    0x568C1115, // v_fmac_f32_e32 v70, v21, v8
    0x568C1316, // v_fmac_f32_e32 v70, v22, v9
    0x08869140, // v_sub_f32_e32 v67, v64, v72
    0x088E9341, // v_sub_f32_e32 v71, v65, v73
    0xE0781000, 0x80064002, // buffer_store_dwordx4 v[64:67], v2, s[24:27], 0 offen
    0xE0781010, 0x80064402, // buffer_store_dwordx4 v[68:71], v2, s[24:27], 0 offen offset:16
    0xBF810000, // s_endpgm
];
pub(super) const SKIN_UNROLLED: &[u32] = &[
    0xBE860304, // s_mov_b32 s6, s4
    0xBE870380, // s_mov_b32 s7, 0
    0xF4080203, 0xFA000000, // s_load_dwordx4 s[8:11], s[6:7], 0x0
    0xF4080303, 0xFA000020, // s_load_dwordx4 s[12:15], s[6:7], 0x20
    0xF4080403, 0xFA000040, // s_load_dwordx4 s[16:19], s[6:7], 0x40
    0xF4080603, 0xFA000060, // s_load_dwordx4 s[24:27], s[6:7], 0x60
    0xD7460001, 0x04010A05, // v_lshl_add_u32 v1, s5, 5, v0
    0x34040285, // v_lshlrev_b32_e32 v2, 5, v1
    0xBF8CC07F, // s_waitcnt lgkmcnt(0)
    0xE0381000, 0x80020402, // buffer_load_dwordx4 v[4:7], v2, s[8:11], 0 offen
    0xE0381010, 0x80020802, // buffer_load_dwordx4 v[8:11], v2, s[8:11], 0 offen offset:16
    0x7E180280, // v_mov_b32_e32 v12, 0
    0x7E1A0280, // v_mov_b32_e32 v13, 0
    0x7E1C0280, // v_mov_b32_e32 v14, 0
    0x7E1E0280, // v_mov_b32_e32 v15, 0
    0x7E200280, // v_mov_b32_e32 v16, 0
    0x7E220280, // v_mov_b32_e32 v17, 0
    0x7E240280, // v_mov_b32_e32 v18, 0
    0x7E260280, // v_mov_b32_e32 v19, 0
    0x7E280280, // v_mov_b32_e32 v20, 0
    0x7E2A0280, // v_mov_b32_e32 v21, 0
    0x7E2C0280, // v_mov_b32_e32 v22, 0
    0x7E2E0280, // v_mov_b32_e32 v23, 0
    0x7E300280, // v_mov_b32_e32 v24, 0
    0x7E320280, // v_mov_b32_e32 v25, 0
    0x7E340280, // v_mov_b32_e32 v26, 0
    0x7E360280, // v_mov_b32_e32 v27, 0
    0x7E380280, // v_mov_b32_e32 v28, 0
    0x7E3A0280, // v_mov_b32_e32 v29, 0
    0x7E3C0280, // v_mov_b32_e32 v30, 0
    0x7E3E0280, // v_mov_b32_e32 v31, 0
    0x7E400280, // v_mov_b32_e32 v32, 0
    0x7E420280, // v_mov_b32_e32 v33, 0
    0x7E440280, // v_mov_b32_e32 v34, 0
    0x7E460280, // v_mov_b32_e32 v35, 0
    0xBF8C3F70, // s_waitcnt vmcnt(0)
    0xD5480024, 0x0221010A, // v_bfe_u32 v36, v10, 0, 8
    0xD5480025, 0x0221010B, // v_bfe_u32 v37, v11, 0, 8
    0x7E4A2325, // v_cvt_f32_ubyte0_e32 v37, v37
    0x104A4AFF, 0x3B808081, // v_mul_f32_e32 v37, 0x3b808081, v37
    0x164C48B0, // v_mul_u32_u24_e32 v38, 48, v36
    0xE0381000, 0x80032826, // buffer_load_dwordx4 v[40:43], v38, s[12:15], 0 offen
    0xE0381010, 0x80032C26, // buffer_load_dwordx4 v[44:47], v38, s[12:15], 0 offen offset:16
    0xE0381020, 0x80033026, // buffer_load_dwordx4 v[48:51], v38, s[12:15], 0 offen offset:32
    0xE0381000, 0x80043426, // buffer_load_dwordx4 v[52:55], v38, s[16:19], 0 offen
    0xE0381010, 0x80043826, // buffer_load_dwordx4 v[56:59], v38, s[16:19], 0 offen offset:16
    0xE0381020, 0x80043C26, // buffer_load_dwordx4 v[60:63], v38, s[16:19], 0 offen offset:32
    0xBF8C3F70, // s_waitcnt vmcnt(0)
    0x56185125, // v_fmac_f32_e32 v12, v37, v40
    0x561A5325, // v_fmac_f32_e32 v13, v37, v41
    0x561C5525, // v_fmac_f32_e32 v14, v37, v42
    0x561E5725, // v_fmac_f32_e32 v15, v37, v43
    0x56205925, // v_fmac_f32_e32 v16, v37, v44
    0x56225B25, // v_fmac_f32_e32 v17, v37, v45
    0x56245D25, // v_fmac_f32_e32 v18, v37, v46
    0x56265F25, // v_fmac_f32_e32 v19, v37, v47
    0x56286125, // v_fmac_f32_e32 v20, v37, v48
    0x562A6325, // v_fmac_f32_e32 v21, v37, v49
    0x562C6525, // v_fmac_f32_e32 v22, v37, v50
    0x562E6725, // v_fmac_f32_e32 v23, v37, v51
    0x56306925, // v_fmac_f32_e32 v24, v37, v52
    0x56326B25, // v_fmac_f32_e32 v25, v37, v53
    0x56346D25, // v_fmac_f32_e32 v26, v37, v54
    0x56366F25, // v_fmac_f32_e32 v27, v37, v55
    0x56387125, // v_fmac_f32_e32 v28, v37, v56
    0x563A7325, // v_fmac_f32_e32 v29, v37, v57
    0x563C7525, // v_fmac_f32_e32 v30, v37, v58
    0x563E7725, // v_fmac_f32_e32 v31, v37, v59
    0x56407925, // v_fmac_f32_e32 v32, v37, v60
    0x56427B25, // v_fmac_f32_e32 v33, v37, v61
    0x56447D25, // v_fmac_f32_e32 v34, v37, v62
    0x56467F25, // v_fmac_f32_e32 v35, v37, v63
    0xD5480024, 0x0221110A, // v_bfe_u32 v36, v10, 8, 8
    0xD5480025, 0x0221110B, // v_bfe_u32 v37, v11, 8, 8
    0x7E4A2325, // v_cvt_f32_ubyte0_e32 v37, v37
    0x104A4AFF, 0x3B808081, // v_mul_f32_e32 v37, 0x3b808081, v37
    0x164C48B0, // v_mul_u32_u24_e32 v38, 48, v36
    0xE0381000, 0x80032826, // buffer_load_dwordx4 v[40:43], v38, s[12:15], 0 offen
    0xE0381010, 0x80032C26, // buffer_load_dwordx4 v[44:47], v38, s[12:15], 0 offen offset:16
    0xE0381020, 0x80033026, // buffer_load_dwordx4 v[48:51], v38, s[12:15], 0 offen offset:32
    0xE0381000, 0x80043426, // buffer_load_dwordx4 v[52:55], v38, s[16:19], 0 offen
    0xE0381010, 0x80043826, // buffer_load_dwordx4 v[56:59], v38, s[16:19], 0 offen offset:16
    0xE0381020, 0x80043C26, // buffer_load_dwordx4 v[60:63], v38, s[16:19], 0 offen offset:32
    0xBF8C3F70, // s_waitcnt vmcnt(0)
    0x56185125, // v_fmac_f32_e32 v12, v37, v40
    0x561A5325, // v_fmac_f32_e32 v13, v37, v41
    0x561C5525, // v_fmac_f32_e32 v14, v37, v42
    0x561E5725, // v_fmac_f32_e32 v15, v37, v43
    0x56205925, // v_fmac_f32_e32 v16, v37, v44
    0x56225B25, // v_fmac_f32_e32 v17, v37, v45
    0x56245D25, // v_fmac_f32_e32 v18, v37, v46
    0x56265F25, // v_fmac_f32_e32 v19, v37, v47
    0x56286125, // v_fmac_f32_e32 v20, v37, v48
    0x562A6325, // v_fmac_f32_e32 v21, v37, v49
    0x562C6525, // v_fmac_f32_e32 v22, v37, v50
    0x562E6725, // v_fmac_f32_e32 v23, v37, v51
    0x56306925, // v_fmac_f32_e32 v24, v37, v52
    0x56326B25, // v_fmac_f32_e32 v25, v37, v53
    0x56346D25, // v_fmac_f32_e32 v26, v37, v54
    0x56366F25, // v_fmac_f32_e32 v27, v37, v55
    0x56387125, // v_fmac_f32_e32 v28, v37, v56
    0x563A7325, // v_fmac_f32_e32 v29, v37, v57
    0x563C7525, // v_fmac_f32_e32 v30, v37, v58
    0x563E7725, // v_fmac_f32_e32 v31, v37, v59
    0x56407925, // v_fmac_f32_e32 v32, v37, v60
    0x56427B25, // v_fmac_f32_e32 v33, v37, v61
    0x56447D25, // v_fmac_f32_e32 v34, v37, v62
    0x56467F25, // v_fmac_f32_e32 v35, v37, v63
    0xD5480024, 0x0221210A, // v_bfe_u32 v36, v10, 16, 8
    0xD5480025, 0x0221210B, // v_bfe_u32 v37, v11, 16, 8
    0x7E4A2325, // v_cvt_f32_ubyte0_e32 v37, v37
    0x104A4AFF, 0x3B808081, // v_mul_f32_e32 v37, 0x3b808081, v37
    0x164C48B0, // v_mul_u32_u24_e32 v38, 48, v36
    0xE0381000, 0x80032826, // buffer_load_dwordx4 v[40:43], v38, s[12:15], 0 offen
    0xE0381010, 0x80032C26, // buffer_load_dwordx4 v[44:47], v38, s[12:15], 0 offen offset:16
    0xE0381020, 0x80033026, // buffer_load_dwordx4 v[48:51], v38, s[12:15], 0 offen offset:32
    0xE0381000, 0x80043426, // buffer_load_dwordx4 v[52:55], v38, s[16:19], 0 offen
    0xE0381010, 0x80043826, // buffer_load_dwordx4 v[56:59], v38, s[16:19], 0 offen offset:16
    0xE0381020, 0x80043C26, // buffer_load_dwordx4 v[60:63], v38, s[16:19], 0 offen offset:32
    0xBF8C3F70, // s_waitcnt vmcnt(0)
    0x56185125, // v_fmac_f32_e32 v12, v37, v40
    0x561A5325, // v_fmac_f32_e32 v13, v37, v41
    0x561C5525, // v_fmac_f32_e32 v14, v37, v42
    0x561E5725, // v_fmac_f32_e32 v15, v37, v43
    0x56205925, // v_fmac_f32_e32 v16, v37, v44
    0x56225B25, // v_fmac_f32_e32 v17, v37, v45
    0x56245D25, // v_fmac_f32_e32 v18, v37, v46
    0x56265F25, // v_fmac_f32_e32 v19, v37, v47
    0x56286125, // v_fmac_f32_e32 v20, v37, v48
    0x562A6325, // v_fmac_f32_e32 v21, v37, v49
    0x562C6525, // v_fmac_f32_e32 v22, v37, v50
    0x562E6725, // v_fmac_f32_e32 v23, v37, v51
    0x56306925, // v_fmac_f32_e32 v24, v37, v52
    0x56326B25, // v_fmac_f32_e32 v25, v37, v53
    0x56346D25, // v_fmac_f32_e32 v26, v37, v54
    0x56366F25, // v_fmac_f32_e32 v27, v37, v55
    0x56387125, // v_fmac_f32_e32 v28, v37, v56
    0x563A7325, // v_fmac_f32_e32 v29, v37, v57
    0x563C7525, // v_fmac_f32_e32 v30, v37, v58
    0x563E7725, // v_fmac_f32_e32 v31, v37, v59
    0x56407925, // v_fmac_f32_e32 v32, v37, v60
    0x56427B25, // v_fmac_f32_e32 v33, v37, v61
    0x56447D25, // v_fmac_f32_e32 v34, v37, v62
    0x56467F25, // v_fmac_f32_e32 v35, v37, v63
    0xD5480024, 0x0221310A, // v_bfe_u32 v36, v10, 24, 8
    0xD5480025, 0x0221310B, // v_bfe_u32 v37, v11, 24, 8
    0x7E4A2325, // v_cvt_f32_ubyte0_e32 v37, v37
    0x104A4AFF, 0x3B808081, // v_mul_f32_e32 v37, 0x3b808081, v37
    0x164C48B0, // v_mul_u32_u24_e32 v38, 48, v36
    0xE0381000, 0x80032826, // buffer_load_dwordx4 v[40:43], v38, s[12:15], 0 offen
    0xE0381010, 0x80032C26, // buffer_load_dwordx4 v[44:47], v38, s[12:15], 0 offen offset:16
    0xE0381020, 0x80033026, // buffer_load_dwordx4 v[48:51], v38, s[12:15], 0 offen offset:32
    0xE0381000, 0x80043426, // buffer_load_dwordx4 v[52:55], v38, s[16:19], 0 offen
    0xE0381010, 0x80043826, // buffer_load_dwordx4 v[56:59], v38, s[16:19], 0 offen offset:16
    0xE0381020, 0x80043C26, // buffer_load_dwordx4 v[60:63], v38, s[16:19], 0 offen offset:32
    0xBF8C3F70, // s_waitcnt vmcnt(0)
    0x56185125, // v_fmac_f32_e32 v12, v37, v40
    0x561A5325, // v_fmac_f32_e32 v13, v37, v41
    0x561C5525, // v_fmac_f32_e32 v14, v37, v42
    0x561E5725, // v_fmac_f32_e32 v15, v37, v43
    0x56205925, // v_fmac_f32_e32 v16, v37, v44
    0x56225B25, // v_fmac_f32_e32 v17, v37, v45
    0x56245D25, // v_fmac_f32_e32 v18, v37, v46
    0x56265F25, // v_fmac_f32_e32 v19, v37, v47
    0x56286125, // v_fmac_f32_e32 v20, v37, v48
    0x562A6325, // v_fmac_f32_e32 v21, v37, v49
    0x562C6525, // v_fmac_f32_e32 v22, v37, v50
    0x562E6725, // v_fmac_f32_e32 v23, v37, v51
    0x56306925, // v_fmac_f32_e32 v24, v37, v52
    0x56326B25, // v_fmac_f32_e32 v25, v37, v53
    0x56346D25, // v_fmac_f32_e32 v26, v37, v54
    0x56366F25, // v_fmac_f32_e32 v27, v37, v55
    0x56387125, // v_fmac_f32_e32 v28, v37, v56
    0x563A7325, // v_fmac_f32_e32 v29, v37, v57
    0x563C7525, // v_fmac_f32_e32 v30, v37, v58
    0x563E7725, // v_fmac_f32_e32 v31, v37, v59
    0x56407925, // v_fmac_f32_e32 v32, v37, v60
    0x56427B25, // v_fmac_f32_e32 v33, v37, v61
    0x56447D25, // v_fmac_f32_e32 v34, v37, v62
    0x56467F25, // v_fmac_f32_e32 v35, v37, v63
    0xD54B0040, 0x043E090C, // v_fma_f32 v64, v12, v4, v15
    0x56800B0D, // v_fmac_f32_e32 v64, v13, v5
    0x56800D0E, // v_fmac_f32_e32 v64, v14, v6
    0xD54B0041, 0x044E0910, // v_fma_f32 v65, v16, v4, v19
    0x56820B11, // v_fmac_f32_e32 v65, v17, v5
    0x56820D12, // v_fmac_f32_e32 v65, v18, v6
    0xD54B0042, 0x045E0914, // v_fma_f32 v66, v20, v4, v23
    0x56840B15, // v_fmac_f32_e32 v66, v21, v5
    0x56840D16, // v_fmac_f32_e32 v66, v22, v6
    0xD54B0048, 0x046E0918, // v_fma_f32 v72, v24, v4, v27
    0x56900B19, // v_fmac_f32_e32 v72, v25, v5
    0x56900D1A, // v_fmac_f32_e32 v72, v26, v6
    0xD54B0049, 0x047E091C, // v_fma_f32 v73, v28, v4, v31
    0x56920B1D, // v_fmac_f32_e32 v73, v29, v5
    0x56920D1E, // v_fmac_f32_e32 v73, v30, v6
    0xD54B004A, 0x048E0920, // v_fma_f32 v74, v32, v4, v35
    0x56940B21, // v_fmac_f32_e32 v74, v33, v5
    0x56940D22, // v_fmac_f32_e32 v74, v34, v6
    0x10880F0C, // v_mul_f32_e32 v68, v12, v7
    0x5688110D, // v_fmac_f32_e32 v68, v13, v8
    0x5688130E, // v_fmac_f32_e32 v68, v14, v9
    0x108A0F10, // v_mul_f32_e32 v69, v16, v7
    0x568A1111, // v_fmac_f32_e32 v69, v17, v8
    0x568A1312, // v_fmac_f32_e32 v69, v18, v9
    0x108C0F14, // v_mul_f32_e32 v70, v20, v7
    0x568C1115, // v_fmac_f32_e32 v70, v21, v8
    0x568C1316, // v_fmac_f32_e32 v70, v22, v9
    0x08869140, // v_sub_f32_e32 v67, v64, v72
    0x088E9341, // v_sub_f32_e32 v71, v65, v73
    0xE0781000, 0x80064002, // buffer_store_dwordx4 v[64:67], v2, s[24:27], 0 offen
    0xE0781010, 0x80064402, // buffer_store_dwordx4 v[68:71], v2, s[24:27], 0 offen offset:16
    0xBF810000, // s_endpgm
];
