//! Human-readable RDNA 2 listing, following emu/rdna2_operands*.py and its LLVM-derived shapes.
mod extensions;
mod graphics;
mod memory;
mod shapes;

use crate::decode::{Instruction, decode, mnemonic};

#[derive(Clone, Copy)]
struct Field {
    name: &'static str,
    kind: &'static str,
    shift: u32,
    mask: u64,
    width: u64,
    style: &'static str,
}
impl Field {
    const fn new(
        name: &'static str,
        kind: &'static str,
        shift: u32,
        mask: u64,
        width: u64,
        style: &'static str,
    ) -> Self {
        Self {
            name,
            kind,
            shift,
            mask,
            width,
            style,
        }
    }
}

fn flag(word: u64, bit: u32) -> bool {
    word >> bit & 1 != 0
}

fn range(prefix: &str, start: u64, width: u64) -> String {
    if width == 1 {
        format!("{prefix}{start}")
    } else {
        format!("{prefix}[{start}:{}]", start + width - 1)
    }
}

fn source(code: u64, width: u64, literal: Option<u32>) -> String {
    match code {
        0..=105 => range("s", code, width),
        106 => if width > 1 { "vcc" } else { "vcc_lo" }.into(),
        107 => "vcc_hi".into(),
        108..=123 => range("ttmp", code - 108, width),
        124 => "m0".into(),
        125 => "null".into(),
        126 => if width > 1 { "exec" } else { "exec_lo" }.into(),
        127 => "exec_hi".into(),
        128..=192 => (code - 128).to_string(),
        193..=208 => format!("-{}", code - 192),
        235..=239 => [
            "src_shared_base",
            "src_shared_limit",
            "src_private_base",
            "src_private_limit",
            "src_pops_exiting_wave_id",
        ][(code - 235) as usize]
            .into(),
        240..=248 => [
            "0.5",
            "-0.5",
            "1.0",
            "-1.0",
            "2.0",
            "-2.0",
            "4.0",
            "-4.0",
            "0.15915494",
        ][(code - 240) as usize]
            .into(),
        251..=254 => {
            ["src_vccz", "src_execz", "src_scc", "src_lds_direct"][(code - 251) as usize].into()
        }
        255 => literal.map_or_else(|| "<littéral>".into(), |x| format!("0x{x:x}")),
        256.. => range("v", code - 256, width),
        _ => format!("<réservé {code}>"),
    }
}

fn immediate(value: u64, style: &str) -> String {
    match style {
        "hwreg" => {
            let id = value & 63;
            let name = shapes::hwreg(id).map_or_else(|| id.to_string(), str::to_owned);
            let offset = value >> 6 & 31;
            let size = (value >> 11 & 31) + 1;
            if offset == 0 && size == 32 {
                format!("hwreg({name})")
            } else {
                format!("hwreg({name}, {offset}, {size})")
            }
        }
        "waitcnt" => {
            let counts = [
                ("vmcnt", (value & 15) | ((value >> 14) << 4), 63),
                ("expcnt", value >> 4 & 7, 7),
                ("lgkmcnt", value >> 8 & 63, 63),
            ];
            let parts: Vec<_> = counts
                .into_iter()
                .filter(|(_, v, max)| v != max)
                .map(|(n, v, _)| format!("{n}({v})"))
                .collect();
            if parts.is_empty() {
                format!("0x{value:x}")
            } else {
                parts.join(" ")
            }
        }
        "hex" => format!("0x{value:x}"),
        "auto" if value > 64 => format!("0x{value:x}"),
        "sdec" => (value as i16).to_string(),
        _ => value.to_string(),
    }
}

fn decorate(mut operand: String, neg: bool, absolute: bool) -> String {
    if absolute {
        operand = format!("|{operand}|");
    }
    if neg {
        operand = format!("-{operand}");
    }
    operand
}

fn render(field: Field, family: &str, word: u64, literal: Option<u32>) -> String {
    let v = (word >> field.shift) & field.mask;
    let w = field.width;
    match field.kind {
        "=" => field.style.into(),
        "literal" => source(255, 1, literal),
        "imm" => immediate(v, field.style),
        "vgpr" => range("v", v, w),
        "sbase" => source(v * 2, w, None),
        "src" => source(v, w, literal),
        "mod0" | "mod1" | "mod2" | "neg0" | "neg1" | "neg2" => {
            let n = u32::from(field.kind.as_bytes()[3] - b'0');
            decorate(
                source(v, w, literal),
                flag(word, 61 + n),
                field.kind.starts_with("mod") && flag(word, 8 + n),
            )
        }
        kind => memory::operand(kind, family, word, v, w),
    }
}

const OUTPUT: [&str; 4] = ["", "mul:2", "mul:4", "div:2"];
fn bits_list(values: impl IntoIterator<Item = u64>) -> String {
    values
        .into_iter()
        .map(|x| x.to_string())
        .collect::<Vec<_>>()
        .join(",")
}
fn join(name: &str, operands: Vec<String>, flags: Vec<String>) -> String {
    [name.to_owned(), operands.join(", "), flags.join(" ")]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
fn label(inst: Instruction, data: &[u8], words: &[u32]) -> Result<String, String> {
    let family = inst.family;
    let lo = u64::from(words[0]);
    let word = lo | (u64::from(*words.get(1).unwrap_or(&0)) << 32);
    if family == "EXP" {
        return Ok(graphics::export(word));
    }
    let Some(name) = mnemonic(family, inst.opcode) else {
        return Ok(format!("{}_op_0x{:X}", family.to_lowercase(), inst.opcode));
    };
    match family {
        "VINTRP" => return Ok(graphics::interp(name, word)),
        "MIMG" => return Ok(graphics::image(name, word, data)),
        "VOP3P" => return Ok(graphics::packed(name, word, words.get(2).copied())),
        _ => (),
    }
    let (vop3b, fields) = shapes::shape(family, inst.opcode);
    let e32 = matches!(family, "VOP1" | "VOP2" | "VOPC");
    if e32 && matches!(lo & 511, 0xe9 | 0xea | 0xf9 | 0xfa) {
        return Ok(extensions::text(family, name, word, fields)?.unwrap_or_else(|| name.to_owned()));
    }
    let wide = matches!(
        family,
        "VOP3" | "SMEM" | "DS" | "MUBUF" | "MTBUF" | "GLOBAL" | "SCRATCH" | "FLAT"
    );
    let literal = words.get(if wide { 2 } else { 1 }).copied();
    let fields = memory::adjust(family, name, word, fields);
    let operands = fields
        .iter()
        .map(|f| render(*f, family, word, literal))
        .collect();
    let mut suffix = memory::suffix(family, name, word);
    let mut label = name.to_owned();
    if family == "VOP3" {
        if matches!(inst.opcode,0..0x140|0x180..0x203) {
            label.push_str("_e64");
        }
        if !vop3b && word >> 11 & 15 != 0 {
            let n = fields
                .iter()
                .filter(|f| matches!(f.name, "src0" | "src1" | "src2"))
                .count();
            let bits = (0..n).map(|i| word >> (11 + i) & 1).chain([word >> 14 & 1]);
            suffix.push(format!("op_sel:[{}]", bits_list(bits)));
        }
        if flag(word, 15) {
            suffix.push("clamp".into());
        }
        if word >> 59 & 3 != 0 {
            suffix.push(OUTPUT[(word >> 59 & 3) as usize].into());
        }
    } else if e32
        && !matches!(
            name,
            "v_madmk_f32"
                | "v_madak_f32"
                | "v_fmamk_f32"
                | "v_fmaak_f32"
                | "v_madmk_f16"
                | "v_madak_f16"
                | "v_fmamk_f16"
                | "v_fmaak_f16"
                | "v_pk_fmac_f16"
                | "v_readfirstlane_b32"
                | "v_swap_b32"
                | "v_swaprel_b32"
                | "v_clrexcp"
                | "v_nop"
                | "v_pipeflush"
        )
    {
        label.push_str("_e32");
    }
    Ok(join(&label, operands, suffix))
}

pub fn lines(data: &[u8]) -> Result<Vec<String>, String> {
    decode(data)
        .into_iter()
        .map(|inst| {
            let bytes = &data[inst.offset..inst.offset + 4 * inst.size];
            let words: Vec<_> = bytes
                .chunks_exact(4)
                .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
                .collect();
            let raw = words
                .iter()
                .map(|w| format!("{w:08X}"))
                .collect::<Vec<_>>()
                .join(" ");
            let label = label(inst, bytes, &words)?;
            Ok(format!("{:06X}  {raw:27} {label}", inst.offset))
        })
        .collect()
}
