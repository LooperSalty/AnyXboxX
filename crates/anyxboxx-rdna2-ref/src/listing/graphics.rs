use super::{bits_list, decorate, flag, join, memory::flags, range, source};

pub(super) fn export(word: u64) -> String {
    let target = word >> 4 & 63;
    let target = match target {
        0..=7 => format!("mrt{target}"),
        8 => "mrtz".into(),
        9 => "null".into(),
        12..=16 => format!("pos{}", target - 12),
        20 => "prim".into(),
        32..=63 => format!("param{}", target - 32),
        _ => format!("invalid_target_{target}"),
    };
    let mut operands = vec![target];
    for i in 0..4 {
        let n = if flag(word, 10) { i / 2 } else { i };
        operands.push(if flag(word, i) {
            format!("v{}", word >> (32 + 8 * n) & 255)
        } else {
            "off".into()
        });
    }
    join(
        "exp",
        operands,
        flags(word, &[(11, "done"), (10, "compr"), (12, "vm")]),
    )
}

pub(super) fn interp(name: &str, word: u64) -> String {
    let v = word & 255;
    let src = if word >> 16 & 3 == 2 {
        match v {
            0 => "p10".into(),
            1 => "p20".into(),
            2 => "p0".into(),
            _ => format!("p{v}"),
        }
    } else {
        format!("v{v}")
    };
    let ch = ["x", "y", "z", "w"][(word >> 8 & 3) as usize];
    format!(
        "{name}_e32 v{}, {src}, attr{}.{ch}",
        word >> 18 & 255,
        word >> 10 & 63
    )
}

pub(super) fn packed(name: &str, word: u64, literal: Option<u32>) -> String {
    let mix = name.starts_with("v_fma_mix");
    let count = if mix
        || ["v_pk_mad_", "v_pk_fma_", "v_dot"]
            .iter()
            .any(|p| name.starts_with(p))
    {
        3
    } else {
        2
    };
    let mut operands = vec![format!("v{}", word & 255)];
    for i in 0..count {
        operands.push(decorate(
            source(word >> (32 + 9 * i) & 511, 1, literal),
            mix && flag(word, 61 + i),
            mix && flag(word, 8 + i),
        ));
    }
    let mut suffix = Vec::new();
    for (label, shifts, default) in [
        ("op_sel", [11, 12, 13], 0),
        ("op_sel_hi", [59, 60, 14], u64::from(!mix)),
        ("neg_lo", [61, 62, 63], 0),
        ("neg_hi", [8, 9, 10], 0),
    ] {
        if mix && label.starts_with("neg") {
            continue;
        }
        let values = shifts[..count as usize].iter().map(|i| word >> i & 1);
        if values.clone().any(|v| v != default) {
            suffix.push(format!("{label}:[{}]", bits_list(values)));
        }
    }
    if flag(word, 15) {
        suffix.push("clamp".into());
    }
    join(name, operands, suffix)
}

const DIMS: [(&str, u64, u64); 8] = [
    ("1D", 1, 2),
    ("2D", 2, 4),
    ("3D", 3, 6),
    ("CUBE", 3, 4),
    ("1D_ARRAY", 2, 2),
    ("2D_ARRAY", 3, 4),
    ("2D_MSAA", 3, 4),
    ("2D_MSAA_ARRAY", 4, 4),
];

fn address_count(name: &str, axes: u64, gradients: u64, a16: bool) -> u64 {
    let tokens: Vec<_> = name.split('_').skip(2).collect();
    let has = |s| tokens.contains(&s);
    let resinfo = name == "image_get_resinfo";
    let coords = if resinfo { 0 } else { axes };
    let components = coords + u64::from(resinfo || has("l") || has("cl") || has("mip"));
    let mut count = if a16 {
        components.div_ceil(2)
    } else {
        components
    };
    count += ["c", "b", "o"].into_iter().filter(|s| has(s)).count() as u64;
    if has("d") || has("cd") {
        count += if has("g16") {
            (gradients / 2).div_ceil(2) * 2
        } else {
            gradients
        };
    }
    count
}

pub(super) fn image(name: &str, word: u64, data: &[u8]) -> String {
    let (dim, axes, gradients) = DIMS[(word >> 3 & 7) as usize];
    let a16 = flag(word, 62);
    let bvh = matches!(
        name,
        "image_bvh_intersect_ray" | "image_bvh64_intersect_ray"
    );
    let addresses = if bvh {
        (if a16 { 8 } else { 11 }) + u64::from(name == "image_bvh64_intersect_ray")
    } else {
        address_count(name, axes, gradients, a16)
    };
    let mask = word >> 8 & 15;
    let mut registers = if name.starts_with("image_gather4") {
        4
    } else {
        u64::from(mask.count_ones().max(1))
    };
    if name.contains("_atomic_") {
        let min = if name.contains("cmpswap") { 2 } else { 1 };
        if registers != min && registers != min * 2 {
            registers = min;
        }
    }
    if flag(word, 63) {
        registers = registers.div_ceil(2);
    }
    registers += u64::from(flag(word, 16));
    let mut operands = vec![range(
        "v",
        word >> 40 & 255,
        if bvh { 4 } else { registers },
    )];
    let start = word >> 32 & 255;
    if word >> 1 & 3 != 0 {
        let regs = std::iter::once(start)
            .chain(data[8..].iter().copied().map(u64::from))
            .take(addresses as usize)
            .map(|r| format!("v{r}"))
            .collect::<Vec<_>>();
        operands.push(format!("[{}]", regs.join(", ")));
    } else {
        operands.push(range(
            "v",
            start,
            if addresses > 12 { 16 } else { addresses },
        ));
    }
    operands.push(source((word >> 48 & 31) * 4, if bvh { 4 } else { 8 }, None));
    if bvh {
        return join(name, operands, flags(word, &[(62, "a16")]));
    }
    if ["image_sample", "image_gather4", "image_get_lod"]
        .iter()
        .any(|p| name.starts_with(p))
    {
        operands.push(source((word >> 53 & 31) * 4, 4, None));
    }
    let mut suffix = Vec::new();
    if mask != 0 {
        suffix.push(format!("dmask:0x{mask:x}"));
    }
    suffix.push(format!("dim:SQ_RSRC_IMG_{dim}"));
    suffix.extend(flags(
        word,
        &[
            (12, "unorm"),
            (13, "glc"),
            (25, "slc"),
            (7, "dlc"),
            (15, "r128"),
            (62, "a16"),
            (16, "tfe"),
            (17, "lwe"),
            (63, "d16"),
        ],
    ));
    join(name, operands, suffix)
}
