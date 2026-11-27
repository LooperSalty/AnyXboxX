//! Shader inspection modes of scarlett.py. The HLSL recompiler has its own port.
use anyxboxx_rdna2_ref::{Wave, decode, run};
use std::{collections::BTreeMap, path::PathBuf, process::ExitCode};

#[derive(clap::Args)]
pub struct Args {
    /// Raw little-endian shader instructions.
    shader: PathBuf,
    /// Disassemble instructions with operands and offsets.
    #[arg(long)]
    liste: bool,
    /// Execute a wave32 and print its nonzero registers.
    #[arg(long)]
    executer: bool,
    /// Legacy option for the removed SPIR-V translator.
    #[arg(long)]
    spirv: bool,
}

fn registers(data: &[u8]) -> Result<(), anyxboxx_rdna2_ref::Error> {
    let mut wave = Wave::new(32)?;
    run(data, &mut wave)?;
    let scalars: Vec<_> = wave.s[..106]
        .iter()
        .enumerate()
        .filter(|(_, x)| **x != 0)
        .map(|(n, x)| format!("s{n}=0x{x:x}"))
        .collect();
    println!(
        "SGPR : {}",
        if scalars.is_empty() {
            "tous nuls".into()
        } else {
            scalars.join(", ")
        }
    );
    println!(
        "VCC=0x{:08x} EXEC=0x{:08x} SCC={}",
        wave.vcc()?,
        wave.exec()?,
        u8::from(wave.scc)
    );
    for (n, values) in wave.v.iter().enumerate().skip(1) {
        if values.iter().any(|v| *v != 0) {
            let lanes: Vec<_> = values[..4].iter().map(|x| format!("{x:08x}")).collect();
            println!("v{n} (lignes 0-3) : {}", lanes.join(" "));
        }
    }
    Ok(())
}

pub fn command(args: Args) -> ExitCode {
    if args.spirv {
        eprintln!("Erreur : --spirv : le traducteur SPIR-V a été retiré (#262) ; utilisez --hlsl");
        return 2.into();
    }
    let data = match std::fs::read(&args.shader) {
        Ok(data) => data,
        Err(error) => {
            eprintln!("Erreur : {} : {error}", args.shader.display());
            return ExitCode::FAILURE;
        }
    };
    if args.liste {
        match anyxboxx_rdna2_ref::listing::lines(&data) {
            Ok(lines) => println!("{}", lines.join("\n")),
            Err(error) => {
                eprintln!("Arrêt : {error}");
                return ExitCode::FAILURE;
            }
        }
    } else if args.executer {
        if let Err(error) = registers(&data) {
            eprintln!("Arrêt : {error}");
            return ExitCode::FAILURE;
        }
    } else {
        let instructions = decode::decode(&data);
        let mut counts = BTreeMap::new();
        for instruction in &instructions {
            *counts.entry(instruction.family).or_insert(0usize) += 1;
        }
        for (family, count) in counts {
            println!("{family:10} {count}");
        }
        println!("Total : {} instructions", instructions.len());
    }
    ExitCode::SUCCESS
}
