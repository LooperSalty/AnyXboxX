//! `anyxboxx`: AnyXboxX's command line in Rust (docs/architecture-rust.md).
//!
//! `anyxboxx run --game-dir <folder> [...]` replaces the former `tools/lancer_victory.ps1`;
//! `anyxboxx rapport <target>` replaces `scarlett.py --rapport`;
//! `anyxboxx traduction <package> [image.svg]` replaces `scarlett.py --traduction`;
//! `anyxboxx carte <package> <image.svg>` replaces `scarlett.py --carte`;
//! `anyxboxx jour-j <package | exe>` replaces `tools/jour_j.py`;
//! `anyxboxx banc <folder>` replaces the static columns of `scarlett.py --banc`.

use std::process::ExitCode;

use anyxboxx::{banc, carte, jour_j, rapport, run, traduction};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "anyxboxx",
    version,
    about = "AnyXboxX: run Xbox Series executables on Windows"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build VIctory and launch it on your own game folder (replaces the former tools/lancer_victory.ps1).
    Run(run::RunArgs),
    /// Compatibility report of an executable or package folder (replaces scarlett.py --rapport).
    Rapport(rapport::RapportArgs),
    /// Translation progress of the console DLLs, and its SVG image (replaces scarlett.py --traduction).
    Traduction(traduction::TraductionArgs),
    /// Progress map of the README, one cell per function (replaces scarlett.py --carte).
    Carte(carte::CarteArgs),
    /// First-launch report of a console executable, written outside the repository (replaces tools/jour_j.py).
    JourJ(jour_j::JourJArgs),
    /// Test bench: imports resolved per module; load and init are not run (replaces scarlett.py --banc).
    Banc(banc::BancArgs),
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Run(args) => launch(&args),
        Command::Rapport(args) => match rapport::rapport(&args) {
            Ok(text) => {
                println!("{text}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("anyxboxx rapport: {error}");
                ExitCode::FAILURE
            }
        },
        Command::Traduction(args) => report("traduction", traduction::traduction(&args)),
        Command::Carte(args) => report("carte", carte::carte(&args)),
        Command::JourJ(args) => report("jour-j", jour_j::jour_j(&args)),
        Command::Banc(args) => report("banc", banc::banc(&args).map(|table| println!("{table}"))),
    }
}

/// Exit code of a report command; its error goes to stderr after `anyxboxx <command>: `.
fn report<E: std::fmt::Display>(command: &str, outcome: Result<(), E>) -> ExitCode {
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("anyxboxx {command}: {error}");
            ExitCode::FAILURE
        }
    }
}

fn launch(args: &run::RunArgs) -> ExitCode {
    let repo = run::find_repo(args.repo.as_deref());
    match run::run(args, repo.as_deref()) {
        // VIctory's exit code passed on whole (a crash code such as 0xC0000005 included).
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("anyxboxx run: {error}");
            ExitCode::FAILURE
        }
    }
}
