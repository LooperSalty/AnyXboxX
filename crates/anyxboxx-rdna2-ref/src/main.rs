//! `anyxboxx-rdna2-ref` : runs an RDNA 2 program and prints the state it ends in, for the parity
//! test against `emu/rdna2_exec.py`.
//!
//! Input (stdin), one directive per line:
//!   `lanes N`          wave size, 32 (default) or 64
//!   `scc 0|1`          SCC starts at this value (default 0)
//!   `s I HEX`          scalar register I starts at HEX (before the program)
//!   `mem ADDR W ...`   the 32-bit words W (hex) written at the 64-bit address ADDR (hex)
//!   `primitive N`      the primitive VINTRP reads (decimal, default 0)
//!   `primitives L:P ...` per-lane primitives (decimal); a lane not listed takes `primitive`
//!   `program W ...`    the program, as 32-bit words in hexadecimal
//! Output (stdout): `steps N`, `pc N`, `scc 0|1`, then `s I HEX` for each non-zero scalar register
//! and `v I HEX ...` (one value per lane) for each vector register with a non-zero lane, then
//! `page ADDR OFF:HEX ...` for each created 4 KiB page (its non-zero dwords, by offset), then
//! `exp TARGET LANE X Y Z W` for each lane an EXP instruction exported (by target, then lane). A run that
//! refuses prints `error KIND` instead (`untranslated`, `operand` or `loop`) and nothing else.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::process::ExitCode;

use anyxboxx_rdna2_ref::{Error, Wave, run};

const VECTOR_REGISTERS: usize = 256;

fn parse_hex(text: &str) -> Result<u32, String> {
    u32::from_str_radix(text, 16).map_err(|_| format!("valeur hexadécimale illisible : {text}"))
}

/// The initial state and the program, read from the directives.
struct Input {
    lanes: usize,
    scc: bool,
    scalars: Vec<(usize, u32)>,
    memory: Vec<(u64, Vec<u32>)>,
    primitive: u64,
    primitives: Option<BTreeMap<usize, u64>>,
    program: Vec<u8>,
}

fn parse(text: &str) -> Result<Input, String> {
    let mut input = Input {
        lanes: 32,
        scc: false,
        scalars: Vec::new(),
        memory: Vec::new(),
        primitive: 0,
        primitives: None,
        program: Vec::new(),
    };
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("lanes") => {
                input.lanes = parts
                    .next()
                    .ok_or("lanes sans valeur")?
                    .parse()
                    .map_err(|_| "lanes illisible")?;
            }
            Some("scc") => {
                input.scc = parts.next() == Some("1");
            }
            Some("s") => {
                let index: usize = parts
                    .next()
                    .ok_or("s sans registre")?
                    .parse()
                    .map_err(|_| "registre illisible")?;
                let value = parse_hex(parts.next().ok_or("s sans valeur")?)?;
                input.scalars.push((index, value));
            }
            Some("mem") => {
                let address = parts.next().ok_or("mem sans adresse")?;
                let address = u64::from_str_radix(address, 16).map_err(|_| "adresse illisible")?;
                let words = parts.map(parse_hex).collect::<Result<Vec<u32>, String>>()?;
                input.memory.push((address, words));
            }
            Some("primitive") => {
                let value = parts.next().ok_or("primitive sans valeur")?;
                input.primitive = value.parse().map_err(|_| "primitive illisible")?;
            }
            Some("primitives") => {
                input.primitives = Some(parse_primitives(parts)?);
            }
            Some("program") => {
                for word in parts {
                    input.program.extend(parse_hex(word)?.to_le_bytes());
                }
            }
            other => return Err(format!("directive inconnue : {other:?}")),
        }
    }
    Ok(input)
}

/// The `L:P` pairs of a `primitives` directive.
fn parse_primitives<'a>(
    parts: impl Iterator<Item = &'a str>,
) -> Result<BTreeMap<usize, u64>, String> {
    parts
        .map(|pair| {
            let (lane, primitive) = pair
                .split_once(':')
                .ok_or_else(|| format!("primitives illisible : {pair}"))?;
            let lane = lane
                .parse()
                .map_err(|_| format!("ligne illisible : {pair}"))?;
            let primitive = primitive
                .parse()
                .map_err(|_| format!("primitive illisible : {pair}"))?;
            Ok((lane, primitive))
        })
        .collect()
}

fn report(wave: &Wave) -> String {
    let mut out = format!(
        "steps {}\npc {}\nscc {}\n",
        wave.steps,
        wave.pc,
        u8::from(wave.scc)
    );
    for (index, value) in wave.s.iter().enumerate() {
        if *value != 0 {
            out.push_str(&format!("s {index} {value:08X}\n"));
        }
    }
    for (index, lanes) in wave.v.iter().enumerate().take(VECTOR_REGISTERS) {
        if lanes.iter().any(|value| *value != 0) {
            let values: Vec<String> = lanes.iter().map(|value| format!("{value:08X}")).collect();
            out.push_str(&format!("v {index} {}\n", values.join(" ")));
        }
    }
    for (address, bytes) in wave.memory.pages() {
        out.push_str(&format!("page {address:016X}"));
        for (index, chunk) in bytes.chunks_exact(4).enumerate() {
            let value = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            if value != 0 {
                out.push_str(&format!(" {:X}:{value:08X}", index * 4));
            }
        }
        out.push('\n');
    }
    for (target, lanes) in &wave.exports {
        for (lane, values) in lanes {
            let values: Vec<String> = values.iter().map(|value| format!("{value:08X}")).collect();
            out.push_str(&format!("exp {target} {lane} {}\n", values.join(" ")));
        }
    }
    out
}

fn execute(input: &Input) -> Result<String, Error> {
    let mut wave = Wave::new(input.lanes)?;
    wave.scc = input.scc;
    wave.primitive = input.primitive;
    wave.primitives = input.primitives.clone();
    for &(index, value) in &input.scalars {
        wave.write_scalar(index as u32, u64::from(value), 1)?;
    }
    for (address, words) in &input.memory {
        wave.memory.write_words(*address, words);
    }
    run(&input.program, &mut wave)?;
    Ok(report(&wave))
}

fn main() -> ExitCode {
    let mut text = String::new();
    if io::stdin().read_to_string(&mut text).is_err() {
        eprintln!("entrée illisible");
        return ExitCode::from(2);
    }
    let input = match parse(&text) {
        Ok(input) => input,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let output = match execute(&input) {
        Ok(report) => report,
        Err(error) => format!("error {}\n", error.kind()),
    };
    let mut stdout = io::stdout().lock();
    if stdout.write_all(output.as_bytes()).is_err() {
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}
