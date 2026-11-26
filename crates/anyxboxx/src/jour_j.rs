//! `anyxboxx jour-j <package folder | game.exe> [--sortie <folder>]`: the first-launch report of
//! `tools/jour_j.py`, written outside the repository (it names functions of the game).
//!
//! The static part (inventory, who provides each import) is the Python report byte for byte.
//! The launch steps differ, because the Unicorn sandbox is dropped (decision D2) and
//! `anyxboxx.exe` with it (decision D4):
//! - "Analyse isolée (--rapport)" becomes "Analyse (anyxboxx rapport)": the same static analysis,
//!   run by this binary, with VIctory's HLE as the table of emulated functions;
//! - "Lancement isolé dans Unicorn (--lancer)" says it is not available;
//! - `--natif` says it is not available either, and points at `anyxboxx run` (VIctory), the
//!   launcher that replaces `anyxboxx.exe`. Nothing of the game is run.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyxboxx_report::jour_j::{Folders, Resolver, static_report};
use anyxboxx_report::resolve_target;
use anyxboxx_report::text::write_text;
use clap::Args;

use crate::emulated::{MeasureError, folders};
use crate::run::find_repo;

const TAIL_LINES: usize = 60;
pub const ISOLATED_UNAVAILABLE: &str = "Non disponible : le bac à sable Unicorn n'est pas porté \
     en Rust (décision D2 de docs/architecture-rust.md). Le jeu se lance avec VIctory : \
     `anyxboxx run --game-dir <dossier>`.";
pub const NATIVE_UNAVAILABLE: &str = "Non disponible : anyxboxx.exe n'est plus le lanceur \
     (décision D4 de docs/architecture-rust.md). Le jeu se lance avec VIctory : \
     `anyxboxx run --game-dir <dossier>`.";

#[derive(Debug, Clone, Args)]
pub struct JourJArgs {
    /// Package folder (MicrosoftGame.Config) or the game's executable.
    pub target: PathBuf,
    /// Report folder (default: %LOCALAPPDATA%\AnyXboxX\jour-j\<date>).
    #[arg(long)]
    pub sortie: Option<PathBuf>,
    /// Native launch: not available in Rust (decision D4); the report says so.
    #[arg(long)]
    pub natif: bool,
    /// Duration of the native launch in seconds (kept for the Python options; unused).
    #[arg(long, default_value_t = 60)]
    pub duree: u64,
    /// Table of emulated functions for the analysis step (default: VIctory's HLE).
    #[arg(long)]
    pub emulated_list: Option<PathBuf>,
    /// Repository root (default: found from the binary or the current folder).
    #[arg(long)]
    pub repo: Option<PathBuf>,
}

/// The folders of jour_j.py: package, `ANYXBOXX_RUNTIME`, `ANYXBOXX_TOMBOX_DIR` (default
/// third_party/tombox/target/release) and `%SystemRoot%\System32`.
fn provider_folders(package: &Path, repo: &Path) -> Folders {
    let tombox = repo
        .join("third_party")
        .join("tombox")
        .join("target")
        .join("release");
    let system_root =
        std::env::var_os("SystemRoot").map_or(PathBuf::from(r"C:\Windows"), PathBuf::from);
    Folders {
        package: package.to_path_buf(),
        runtime: folders::runtime(repo),
        tombox: folders::from_env("ANYXBOXX_TOMBOX_DIR", tombox),
        system32: system_root.join("System32"),
    }
}

/// `## title`, the outcome and the last lines of the output, as `isolated_steps` writes them.
fn analysis_step(args: &JourJArgs, repo: &Path) -> Result<Vec<String>, MeasureError> {
    let mut command =
        Command::new(std::env::current_exe().map_err(|e| MeasureError::Io("anyxboxx".into(), e))?);
    command.arg("rapport").arg(&args.target).current_dir(repo);
    if let Some(list) = &args.emulated_list {
        command.arg("--emulated-list").arg(list);
    }
    let started = Instant::now();
    let output = command
        .output()
        .map_err(|e| MeasureError::Io("anyxboxx rapport".into(), e))?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = text.trim().lines().collect();
    let tail = lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n");
    let code = output
        .status
        .code()
        .map_or("aucun".into(), |c| c.to_string());
    Ok(vec![
        "## Analyse (anyxboxx rapport)".into(),
        String::new(),
        format!(
            "code de sortie {code}, {:.1} s.",
            started.elapsed().as_secs_f64()
        ),
        String::new(),
        "```".into(),
        tail,
        "```".into(),
        String::new(),
    ])
}

fn unavailable(title: &str, text: &str) -> Vec<String> {
    vec![title.into(), String::new(), text.into(), String::new()]
}

/// The whole report.
pub fn report(args: &JourJArgs) -> Result<String, MeasureError> {
    let repo = find_repo(args.repo.as_deref()).ok_or(MeasureError::NoRepo)?;
    let exe = resolve_target(&args.target)?;
    if !exe.is_file() {
        return Err(MeasureError::Missing(exe));
    }
    let package = exe.parent().unwrap_or(Path::new("."));
    let resolver = Resolver::new(provider_folders(package, &repo));
    let mut lines = static_report(&exe, &resolver)?;
    lines.extend(analysis_step(args, &repo)?);
    lines.extend(unavailable(
        "## Lancement isolé dans Unicorn (--lancer)",
        ISOLATED_UNAVAILABLE,
    ));
    if args.natif {
        lines.extend(unavailable(
            "## Lancement natif (anyxboxx.exe)",
            NATIVE_UNAVAILABLE,
        ));
    }
    Ok(lines.join("\n") + "\n")
}

/// Writes `rapport.md` in the report folder, prints its summary line and its path.
pub fn jour_j(args: &JourJArgs) -> Result<(), MeasureError> {
    let output = args.sortie.clone().unwrap_or_else(default_folder);
    std::fs::create_dir_all(&output).map_err(|e| MeasureError::Io(output.clone(), e))?;
    if args.natif {
        eprintln!(
            "anyxboxx jour-j: --natif is not available in Rust (decision D4); the report says so. \
             Launch the game with `anyxboxx run`."
        );
    }
    let text = report(args)?;
    let path = output.join("rapport.md");
    write_text(&path, &text).map_err(|e| MeasureError::Io(path.clone(), e))?;
    println!("{}", text.lines().nth(2).unwrap_or_default());
    println!("Rapport : {}", path.display());
    Ok(())
}

/// `%LOCALAPPDATA%\AnyXboxX\jour-j\<YYYY-mm-dd_HHMMSS>` (home folder without LOCALAPPDATA).
fn default_folder() -> PathBuf {
    let base = ["LOCALAPPDATA", "USERPROFILE", "HOME"]
        .iter()
        .find_map(std::env::var_os)
        .map_or(PathBuf::from("."), PathBuf::from);
    base.join("AnyXboxX")
        .join("jour-j")
        .join(crate::clock::stamp())
}
