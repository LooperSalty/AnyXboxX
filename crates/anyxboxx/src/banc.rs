//! `anyxboxx banc <folder> [DLL folders...]`: the test bench of `scarlett.py --banc`.
//!
//! Every PE32+ module of the folder with its imports and the share resolved: the same numbers as
//! Python. The Python bench then loads each module in the Unicorn sandbox and runs its
//! initialisation (DllMain, or the whole executable). That sandbox is dropped (decision D2 of
//! docs/architecture-rust.md) and no Rust replacement runs module code outside a game launch, so
//! the "Chargé" and "Init." columns say `n/d` and the footer says why: nothing is reported as
//! loaded or initialised without having been run.

use std::path::{Path, PathBuf};

use anyxboxx_report::bench;
use clap::Args;

use crate::emulated::{Emulated, MeasureError};

#[derive(Debug, Clone, Args)]
pub struct BancArgs {
    /// Folder of DLL and EXE modules.
    pub folder: PathBuf,
    /// More folders of DLLs the modules may load (e.g. Visual C++ redistributables).
    pub extra_dirs: Vec<PathBuf>,
    /// Table of emulated functions, one `family!function` per line (default: VIctory's HLE).
    #[arg(long)]
    pub emulated_list: Option<PathBuf>,
}

/// The bench table.
pub fn banc(args: &BancArgs) -> Result<String, MeasureError> {
    let emulated = Emulated::load(args.emulated_list.as_deref())?;
    let contains = |dll: &str, function: &str| emulated.contains(dll, function);
    let extra: Vec<&Path> = args.extra_dirs.iter().map(PathBuf::as_path).collect();
    Ok(bench::table(&bench::rows(&args.folder, &extra, &contains)))
}
