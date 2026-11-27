//! 2D image operations on the linear T# descriptors modelled by the interpreter.
use anyxboxx_rdna2_ref::image_desc::{self as isa, Fields, Op};

const ACTIVE: &str = "((s[126] >> lane) & 1u) != 0";
const COMPONENTS: [&str; 4] = ["x", "y", "z", "w"];

fn descriptor(label: &str, r: usize) -> String {
    format!(
        "uint4 {label} = uint4(s[{r}], s[{}], s[{}], s[{}]);",
        r + 1,
        r + 2,
        r + 3
    )
}

pub fn statements(name: &str, bytes: &[u8]) -> Result<Vec<String>, String> {
    let words: Vec<_> = bytes
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    let f = isa::instruction_fields(&words);
    let extended = matches!(
        name.strip_prefix("image_gather4"),
        Some(
            "_c" | "_c_lz"
                | "_c_l"
                | "_c_o"
                | "_c_lz_o"
                | "_c_l_o"
                | "_cl"
                | "_b"
                | "_b_cl"
                | "_cl_o"
                | "_b_o"
                | "_b_cl_o"
                | "_c_cl"
                | "_c_b"
                | "_c_b_cl"
                | "_c_cl_o"
                | "_c_b_o"
                | "_c_b_cl_o"
        )
    );
    let op = isa::check_instruction(if extended { "image_gather4" } else { name }, &f)
        .map_err(|e| format!("{name}: {e}"))?;
    let mut lines = vec![descriptor("t", f.srsrc)];
    let mut mask = f.dmask;
    let addr = isa::address_registers(name, op, &f);
    let d = f.vdata;
    let store = name.starts_with("image_store");
    if op == Op::Atomic {
        if isa::atomic_width(name, mask).map_err(|e| e.to_string())? != 32 {
            return Err(format!("{name}: 64-bit image atomic"));
        }
        let data1 = if isa::is_cmpswap(name) {
            format!("v[{}]", d + 1)
        } else {
            "0u".into()
        };
        let code = match name {
            "image_atomic_swap" => 0,
            "image_atomic_cmpswap" => 1,
            "image_atomic_add" => 2,
            "image_atomic_sub" => 3,
            "image_atomic_smin" => 4,
            "image_atomic_umin" => 5,
            "image_atomic_smax" => 6,
            "image_atomic_umax" => 7,
            "image_atomic_and" => 8,
            "image_atomic_or" => 9,
            "image_atomic_xor" => 10,
            "image_atomic_inc" => 11,
            "image_atomic_dec" => 12,
            "image_atomic_fcmpswap" => 13,
            "image_atomic_fmin" => 14,
            "image_atomic_fmax" => 15,
            _ => return Err(format!("untranslated instruction: {name}")),
        };
        let call = format!(
            "ImageAtomic32(t, v[{}], v[{}], {code}u, v[{d}], {data1})",
            addr[0], addr[1]
        );
        lines.push(if f.glc {
            format!("uint old = {call}; v[{d}] = old;")
        } else {
            format!("{call};")
        });
    } else if name == "image_get_resinfo" {
        lines.push(format!("uint4 res = ImageResInfo(t, v[{}]);", addr[0]));
    } else if op == Op::Direct {
        let mip = addr
            .get(2)
            .map_or_else(|| "0u".into(), |r| format!("v[{r}]"));
        let args = format!("t, v[{}], v[{}], {mip}", addr[0], addr[1]);
        if store {
            let mut regs = d..;
            let packed: Vec<_> = (0..4)
                .map(|i| {
                    if mask >> i & 1 == 0 {
                        "0u".into()
                    } else {
                        format!("v[{}]", regs.next().unwrap())
                    }
                })
                .collect();
            lines.push(format!(
                "ImageStore({args}, {mask}u, uint4({}));",
                packed.join(", ")
            ));
        } else {
            lines.push(format!("uint4 res = ImageLoad({args});"));
        }
    } else {
        lines.push(descriptor("q", f.ssamp));
        lines.push(format!("uint4 res = {};", sample(name, op, &f)?));
        if op == Op::Gather {
            mask = 15;
        }
    }
    let writes = store || op == Op::Atomic;
    if !writes {
        let channels = COMPONENTS
            .iter()
            .enumerate()
            .filter(|(i, _)| mask >> i & 1 != 0);
        for (r, (_, channel)) in (d..).zip(channels) {
            lines.push(format!("v[{r}] = res.{channel};"));
        }
    }
    let mut body = vec![format!("if ({ACTIVE}) {{ {} }}", lines.join(" "))];
    if writes {
        body.push("AllMemoryBarrierWithGroupSync();".into());
    }
    Ok(body)
}

fn sample(name: &str, op: Op, f: &Fields) -> Result<String, String> {
    // ISA Table 44: offset, bias, comparison reference, s, t, then LOD or clamp.
    let has = |suffix| name.split('_').skip(2).any(|part| part == suffix);
    let mut slot = 0;
    let mut register = || {
        let r = if f.nsa != 0 {
            f.vaddr.get(slot).copied()
        } else {
            Some(f.vaddr[0] + slot)
        };
        slot += 1;
        r.ok_or_else(|| format!("{name}: missing NSA address"))
    };
    let offset = if has("o") {
        format!("ImageOffset(v[{}])", register()?)
    } else {
        "int2(0, 0)".into()
    };
    if has("b") {
        register()?;
    }
    let z = has("c").then(&mut register).transpose()?;
    let (s, t) = (register()?, register()?);
    let lod = (has("l") || has("cl")).then(register).transpose()?;
    let coords = format!("v[{s}], v[{t}], {}u", u32::from(f.unorm));
    if op == Op::Gather {
        return Ok(if let Some(z) = z {
            if f.dmask != 1 {
                return Err(format!("{name}: compare gather requires red"));
            }
            format!("GatherCompare(t, q, v[{z}], {coords}, {offset})")
        } else {
            format!(
                "ImageGather4(t, q, {coords}, {}u, {offset})",
                f.dmask.trailing_zeros()
            )
        });
    }
    let args = format!(
        "{coords}, {offset}, {}u, {}",
        u32::from(lod.is_some()),
        lod.map_or_else(|| "0u".into(), |r| format!("v[{r}]"))
    );
    Ok(if let Some(z) = z {
        format!("ImageSampleCompare(t, q, v[{z}], {args})")
    } else {
        format!("ImageSampleColor(t, q, {args})")
    })
}
