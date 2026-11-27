//! HLSL statements for the wide operations supported by the Python emitter.
pub fn template(name: &str) -> Option<&'static str> {
    Some(match name {
        "v_add_f64" => {
            "if (((s[126] >> lane) & 1u) != 0) { uint lo, hi; asuint(({a} + {b}), lo, hi); v[{dest}] = lo; v[{next}] = hi; }"
        }
        "v_ashrrev_i64" => {
            "if (((s[126] >> lane) & 1u) != 0) { uint n = ({a}) & 63u, l = {b_lo}, h = {b_hi}; if (n >= 32u) { l = asuint(asint(h) >> (n - 32u)); h = asuint(asint(h) >> 31); } else if (n != 0u) { l = (l >> n) | (h << (32u - n)); h = asuint(asint(h) >> n); } v[{dest}] = l; v[{next}] = h; }"
        }
        "v_cvt_f32_f64" => "if (((s[126] >> lane) & 1u) != 0) v[{dest}] = asuint((float){a});",
        "v_cvt_f64_f32" => {
            "if (((s[126] >> lane) & 1u) != 0) { uint cw = {a}, cs = cw & 0x80000000u, ce = cw >> 23 & 0xFFu, cm = cw & 0x7FFFFFu, cx; if (ce == 0xFFu) cx = 0x7FF00000u | (cm != 0u ? 0x80000u : 0u); else if (ce != 0u) cx = (ce + 896u) << 20; else if (cm == 0u) cx = 0u; else { uint cn = 23u - firstbithigh(cm); cm = cm << cn & 0x7FFFFFu; cx = (897u - cn) << 20; } v[{dest}] = cm << 29; v[{next}] = cs | cx | cm >> 3; }"
        }
        "v_cvt_f64_i32" => {
            "if (((s[126] >> lane) & 1u) != 0) { uint lo, hi; asuint((asint({a}) < 0 ? -(double)(0u - {a}) : (double){a}), lo, hi); v[{dest}] = lo; v[{next}] = hi; }"
        }
        "v_cvt_f64_u32" => {
            "if (((s[126] >> lane) & 1u) != 0) { uint lo, hi; asuint((double){a}, lo, hi); v[{dest}] = lo; v[{next}] = hi; }"
        }
        "v_cvt_i32_f64" => {
            "if (((s[126] >> lane) & 1u) != 0) v[{dest}] = SatF64ToI32({a_lo}, {a_hi});"
        }
        "v_cvt_u32_f64" => {
            "if (((s[126] >> lane) & 1u) != 0) v[{dest}] = SatF64ToU32({a_lo}, {a_hi});"
        }
        "v_fma_f64" => {
            "if (((s[126] >> lane) & 1u) != 0) { uint lo, hi; asuint(fma({a}, {b}, {c}), lo, hi); v[{dest}] = lo; v[{next}] = hi; }"
        }
        "v_lshlrev_b64" => {
            "if (((s[126] >> lane) & 1u) != 0) { uint n = ({a}) & 63u, l = {b_lo}, h = {b_hi}; if (n >= 32u) { h = l << (n - 32u); l = 0u; } else if (n != 0u) { h = (h << n) | (l >> (32u - n)); l = l << n; } v[{dest}] = l; v[{next}] = h; }"
        }
        "v_lshrrev_b64" => {
            "if (((s[126] >> lane) & 1u) != 0) { uint n = ({a}) & 63u, l = {b_lo}, h = {b_hi}; if (n >= 32u) { l = h >> (n - 32u); h = 0u; } else if (n != 0u) { l = (l >> n) | (h << (32u - n)); h = h >> n; } v[{dest}] = l; v[{next}] = h; }"
        }
        "v_max_f64" => {
            "if (((s[126] >> lane) & 1u) != 0) { uint lo, hi; asuint(max({a}, {b}), lo, hi); v[{dest}] = lo; v[{next}] = hi; }"
        }
        "v_min_f64" => {
            "if (((s[126] >> lane) & 1u) != 0) { uint lo, hi; asuint(min({a}, {b}), lo, hi); v[{dest}] = lo; v[{next}] = hi; }"
        }
        "v_mul_f64" => {
            "if (((s[126] >> lane) & 1u) != 0) { uint lo, hi; asuint(({a} * {b}), lo, hi); v[{dest}] = lo; v[{next}] = hi; }"
        }
        _ => return None,
    })
}
