//! HLSL expressions for the integer 16-bit operations shared by VOP3 and VOP3P.
pub fn operation(name: &str) -> Option<(&'static str, Option<&'static str>, usize)> {
    Some(match name {
        "add_u16" => ("({a} + {b})", Some("min({a} + {b}, 0xFFFFu)"), 2),
        "sub_u16" => ("({a} - {b})", Some("({a} > {b} ? {a} - {b} : 0u)"), 2),
        "add_i16" => ("({a} + {b})", Some("AddSatI16({a}, {b}, false)"), 2),
        "sub_i16" => ("({a} - {b})", Some("AddSatI16({a}, {b}, true)"), 2),
        "mul_lo_u16" => ("({a} * {b})", None, 2),
        "lshlrev_b16" => ("({b} << ({a} & 15u))", None, 2),
        "lshrrev_b16" => ("({b} >> ({a} & 15u))", None, 2),
        "ashrrev_i16" => ("asuint(Sext16({b}) >> int({a} & 15u))", None, 2),
        "max_u16" => ("max({a}, {b})", None, 2),
        "max_i16" => ("asuint(max(Sext16({a}), Sext16({b})))", None, 2),
        "min_u16" => ("min({a}, {b})", None, 2),
        "min_i16" => ("asuint(min(Sext16({a}), Sext16({b})))", None, 2),
        "mad_u16" => (
            "({a} * {b} + {c})",
            Some("min({a} * {b} + {c}, 0xFFFFu)"),
            3,
        ),
        "mad_i16" => (
            "asuint(Sext16({a}) * Sext16({b}) + Sext16({c}))",
            Some("SatI16(Sext16({a}) * Sext16({b}) + Sext16({c}))"),
            3,
        ),
        "min3_u16" => ("min(min({a}, {b}), {c})", None, 3),
        "min3_i16" => (
            "asuint(min(min(Sext16({a}), Sext16({b})), Sext16({c})))",
            None,
            3,
        ),
        "max3_u16" => ("max(max({a}, {b}), {c})", None, 3),
        "max3_i16" => (
            "asuint(max(max(Sext16({a}), Sext16({b})), Sext16({c})))",
            None,
            3,
        ),
        "med3_u16" => ("max(min({a}, {b}), min(max({a}, {b}), {c}))", None, 3),
        "med3_i16" => (
            "asuint(max(min(Sext16({a}), Sext16({b})), min(max(Sext16({a}), Sext16({b})), Sext16({c}))))",
            None,
            3,
        ),
        _ => return None,
    })
}
