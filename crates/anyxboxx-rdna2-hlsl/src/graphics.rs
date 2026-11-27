//! Vertex and pixel HLSL using the record layout shared with the graphics interpreter.
use anyxboxx_rdna2_ref::{
    decode::{Instruction, decode, mnemonic},
    export, interp,
};
use std::{collections::BTreeSet, fmt::Write};

const ACTIVE: &str = "((s[126] >> lane) & 1u) != 0";
const PIXEL_RECORDS: usize = 0x58000;
const PIXEL_STRIDE: usize = (4 + 8 * 4) * 4;

#[derive(Clone, Copy)]
pub enum Stage {
    Vertex,
    Pixel {
        width: u32,
        multi_lane: Option<bool>,
    },
}

fn needs_wave(inst: &Instruction) -> bool {
    let name = mnemonic(inst.family, inst.opcode).unwrap_or("");
    inst.family == "VOPC"
        || matches!(
            name,
            "v_readlane_b32"
                | "v_readfirstlane_b32"
                | "v_writelane_b32"
                | "v_mbcnt_lo_u32_b32"
                | "v_mbcnt_hi_u32_b32"
                | "s_wqm_b32"
                | "global_load_dword_addtid"
                | "global_store_dword_addtid"
                | "ds_read_addtid_b32"
                | "ds_write_addtid_b32"
        )
        || [
            "v_cmp_",
            "v_cmpx_",
            "s_and_saveexec",
            "s_or_saveexec",
            "s_andn2_saveexec",
            "s_xor_saveexec",
        ]
        .iter()
        .any(|p| name.starts_with(p))
}

fn header(stage: &str, multi: bool) -> String {
    let helpers = if multi {
        include_str!("pixel_wave_helpers.hlsl")
    } else {
        include_str!("pixel_single_helpers.hlsl")
    };
    format!(
        "// Généré par AnyXboxX (emu/rdna2_hlsl_graphics.py) : {stage} shader RDNA 2 recompilé en HLSL.\n#define END_PC 0xFFFFFFFFu\nRWByteAddressBuffer g_memory : register(u0);\n\n{helpers}{}\n",
        super::PRELUDE
    )
}

fn pair(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes[..8].try_into().unwrap())
}

fn pixel_header(width: u32, multi: bool, targets: &BTreeSet<u32>) -> String {
    let mut out = header("pixel", multi);
    let returned = if targets.is_empty() {
        "void"
    } else {
        let members = targets
            .iter()
            .map(|t| format!("float4 t{t} : SV_Target{t};"))
            .collect::<Vec<_>>()
            .join(" ");
        writeln!(out, "struct PSOut {{ {members} }};").unwrap();
        "PSOut"
    };
    writeln!(out,"{returned} main(float4 position : SV_Position, float2 bary : TEXCOORD0, uint primitive : SV_PrimitiveID) {{").unwrap();
    out.push_str(if multi {
        include_str!("pixel_wave_registers.hlsl")
    } else {
        include_str!("pixel_single_registers.hlsl")
    });
    if !targets.is_empty() {
        out.push_str("  PSOut o_out;\n");
        for t in targets {
            writeln!(out, "  o_out.t{t} = float4(0.0f, 0.0f, 0.0f, 0.0f);").unwrap();
        }
    }
    writeln!(out,"  uint record = {PIXEL_RECORDS}u + (uint(position.y) * {width}u + uint(position.x)) * {PIXEL_STRIDE}u;").unwrap();
    out.push_str("  v[0] = asuint(bary.x);\n  v[1] = asuint(bary.y);\n  ");
    if multi {
        out.push_str("if (!IsHelperLane()) ");
    }
    out.push_str("g_memory.Store4(record, uint4(1u, v[0], v[1], primitive));\n");
    out
}

fn export_value(stage: Stage, word: u64) -> Result<Vec<String>, String> {
    let f = export::fields(word);
    let name = export::target_name(f.target);
    if f.compressed {
        return Err(format!("exp {name}: compressed export"));
    }
    let (offset, output) = match (stage, f.target) {
        (Stage::Vertex, 12) => (0, " o_out.position = asfloat(e);".to_owned()),
        (Stage::Vertex, 32..=39) => ((1 + f.target - 32) * 16, String::new()),
        (Stage::Pixel { .. }, 0..=7) => (
            (1 + f.target) * 16,
            format!(" o_out.t{} = asfloat(e);", f.target),
        ),
        (Stage::Pixel { .. }, _) if name == "null" => return Ok(vec![]),
        _ => return Err(format!("exp {name}: unsupported stage target")),
    };
    let values = f
        .sources
        .iter()
        .enumerate()
        .map(|(i, r)| {
            if f.enable & (1 << i) != 0 {
                format!("v[{r}]")
            } else {
                "0u".to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    Ok(vec![format!(
        "if ({ACTIVE}) {{ uint4 e = uint4({values}); g_memory.Store4(record + {offset}u, e);{output} }}"
    )])
}

fn interpolate(name: &str, word: u32) -> Result<Vec<String>, String> {
    let f = interp::fields(word);
    if f.attr >= interp::PARAMETER_COUNT {
        return Err(format!("{name}: unsupported attribute {}", f.attr));
    }
    let offset = interp::parameter_address(0, f.attr, f.chan);
    let load = format!(
        "uint3 p = g_memory.Load3({offset}u + primitive * {}u);",
        interp::PRIMITIVE_STRIDE
    );
    let d = f.vdst;
    let s = f.vsrc;
    let expr = match name {
        "v_interp_p1_f32" => format!(
            "precise float m = asfloat(p.y) * asfloat(v[{s}]); precise float r = m + asfloat(p.x); v[{d}] = asuint(r);"
        ),
        "v_interp_p2_f32" => format!(
            "precise float m = asfloat(p.z) * asfloat(v[{s}]); precise float r = m + asfloat(v[{d}]); v[{d}] = asuint(r);"
        ),
        "v_interp_mov_f32" => format!(
            "v[{d}] = {};",
            ["p.y", "p.z", "p.x"].get(s).unwrap_or(&"0u")
        ),
        _ => return Err(format!("untranslated instruction: {name}")),
    };
    Ok(vec![format!("if ({ACTIVE}) {{ {load} {expr} }}")])
}

/// Translate a graphics shader with an optional initial scalar register file.
pub fn translate(
    data: &[u8],
    stage: Stage,
    sgprs: &std::collections::BTreeMap<u32, u32>,
) -> Result<String, String> {
    if !data.len().is_multiple_of(4) {
        return Err("truncated instruction".into());
    }
    let instructions = decode(data);
    let (mut out, finish) = match stage {
        Stage::Vertex => (
            header("vertex", false) + include_str!("vertex_init.hlsl"),
            "return o_out;",
        ),
        Stage::Pixel { width, multi_lane } => {
            let targets = instructions
                .iter()
                .filter(|i| i.family == "EXP")
                .map(|i| export::fields(pair(&data[i.offset..])).target)
                .filter(|t| *t < 8)
                .collect::<BTreeSet<_>>();
            let multi = multi_lane.unwrap_or_else(|| instructions.iter().any(needs_wave));
            (
                pixel_header(width, multi, &targets),
                if targets.is_empty() {
                    "return;"
                } else {
                    "return o_out;"
                },
            )
        }
    };
    for (register, value) in sgprs {
        if *register >= 128 {
            return Err(format!("invalid scalar register: s{register}"));
        }
        writeln!(out, "  s[{register}] = 0x{value:X}u;").unwrap();
    }
    super::control_flow::emit(
        data,
        &instructions,
        32,
        finish,
        &mut out,
        |name, inst, bytes| match inst.family {
            "EXP" => export_value(stage, pair(bytes)),
            "VINTRP" if matches!(stage, Stage::Pixel { .. }) => {
                interpolate(name, u32::from_le_bytes(bytes[..4].try_into().unwrap()))
            }
            "DS" | "VINTRP" => Err(format!("{name}: not supported in this shader stage")),
            _ => super::statements(name, inst, bytes),
        },
    )?;
    writeln!(out, "  {finish}\n}}").unwrap();
    Ok(out)
}
