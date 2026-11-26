//! `anyxboxx carte <package folder> <image.svg>`: the progress map of `scarlett.py --carte`
//! (`docs/images/carte-avancee.svg`), one cell per function, then its badges on stdout.
//!
//! Same inputs as the Python command (see `traduction`): package, runtime and TomBox DLLs, the
//! GDK tables of `runtime/xgameruntime`, and the RDNA 2 table.

use std::path::PathBuf;

use anyxboxx_report::gdk::{self, GdkSources};
use anyxboxx_report::gpu::GpuCoverage;
use anyxboxx_report::status_map::{self, MapInputs, Status};
use anyxboxx_report::text::write_text;
use clap::Args;

use crate::emulated::{Emulated, MeasureError, folders};
use crate::run::find_repo;

#[derive(Debug, Clone, Args)]
pub struct CarteArgs {
    /// Package folder (its DLL and EXE modules are measured).
    pub package: PathBuf,
    /// SVG image to write (its folder is created).
    pub svg: PathBuf,
    /// Table of emulated functions, one `family!function` per line (default: VIctory's HLE).
    #[arg(long)]
    pub emulated_list: Option<PathBuf>,
    /// RDNA 2 coverage table (default: the built-in snapshot of the Python measure).
    #[arg(long)]
    pub gpu_coverage: Option<PathBuf>,
    /// Repository root (default: found from the binary or the current folder).
    #[arg(long)]
    pub repo: Option<PathBuf>,
}

/// Everything the map shows.
pub fn status(args: &CarteArgs) -> Result<Status, MeasureError> {
    let repo = find_repo(args.repo.as_deref()).ok_or(MeasureError::NoRepo)?;
    let emulated = Emulated::load(args.emulated_list.as_deref())?;
    let gpu = GpuCoverage::load(args.gpu_coverage.as_deref()).map_err(MeasureError::Gpu)?;
    let sources = GdkSources::in_folder(&folders::gdk_sources(&repo));
    let gdk = gdk::read(&sources).map_err(|e| MeasureError::Io(sources.table.clone(), e))?;
    let contains = |dll: &str, function: &str| emulated.contains(dll, function);
    Ok(status_map::collect(&MapInputs {
        package: &args.package,
        runtime: &folders::runtime(&repo),
        tombox: &folders::tombox(&repo),
        gdk: &gdk,
        gpu: &gpu,
        emulated: &contains,
    }))
}

/// Writes the map, then prints the badges (`label : value`).
pub fn carte(args: &CarteArgs) -> Result<(), MeasureError> {
    let status = status(args)?;
    if let Some(folder) = args.svg.parent().filter(|f| !f.as_os_str().is_empty()) {
        std::fs::create_dir_all(folder).map_err(|e| MeasureError::Io(folder.into(), e))?;
    }
    write_text(&args.svg, &status_map::render(&status))
        .map_err(|e| MeasureError::Io(args.svg.clone(), e))?;
    for badge in &status.badges {
        println!("{} : {}", badge.label, badge.value);
    }
    Ok(())
}
