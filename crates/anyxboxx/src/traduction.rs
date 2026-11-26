//! `anyxboxx traduction <package folder> [image.svg]`: the translation progress table of
//! `scarlett.py --traduction`, and the README image (`docs/images/traduction.svg`) when a path
//! is given.
//!
//! Same inputs as the Python command: the package's modules, the runtime DLLs
//! (`ANYXBOXX_RUNTIME`), TomBox's DLLs (`ANYXBOXX_TOMBOX_DIR`) and the GDK tables of
//! `runtime/xgameruntime`. The share of system functions is the static one: the Python bench
//! runs each module in the Unicorn sandbox first, but that run does not change this number.

use std::path::{Path, PathBuf};

use anyxboxx_report::bench;
use anyxboxx_report::gdk::{self, GdkSources};
use anyxboxx_report::gpu::GpuCoverage;
use anyxboxx_report::text::write_text;
use anyxboxx_report::translation::{self, Area};
use clap::Args;

use crate::emulated::{Emulated, MeasureError, folders};
use crate::run::find_repo;

#[derive(Debug, Clone, Args)]
pub struct TraductionArgs {
    /// Package folder (its DLL and EXE modules are measured).
    pub package: PathBuf,
    /// Write the SVG bar chart here too.
    pub svg: Option<PathBuf>,
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

/// Every area, in the order of `print_translation`.
pub fn areas(args: &TraductionArgs) -> Result<Vec<Area>, MeasureError> {
    let repo = find_repo(args.repo.as_deref()).ok_or(MeasureError::NoRepo)?;
    let emulated = Emulated::load(args.emulated_list.as_deref())?;
    let gpu = GpuCoverage::load(args.gpu_coverage.as_deref()).map_err(MeasureError::Gpu)?;
    let contains = |dll: &str, function: &str| emulated.contains(dll, function);
    let rows = bench::rows(&args.package, &[], &contains);
    let sources = GdkSources::in_folder(&folders::gdk_sources(&repo));
    let gdk = gdk::read(&sources).map_err(|e| MeasureError::Io(sources.table.clone(), e))?;
    let runtime = folders::runtime(&repo);
    let tombox = folders::tombox(&repo);
    let mut areas = vec![
        translation::cpu_area(),
        translation::gpu_disassembly_area(&gpu),
        translation::gpu_area(&gpu),
        translation::functions_area(&rows),
    ];
    areas.extend(translation::measure(
        &args.package,
        Some(&runtime),
        Some(&tombox),
    ));
    areas.extend(translation::static_areas());
    areas.push(translation::gdk_runtime_area(&gdk));
    Ok(areas)
}

/// Prints the table, then writes the image if asked.
pub fn traduction(args: &TraductionArgs) -> Result<(), MeasureError> {
    let areas = areas(args)?;
    println!("{}", translation::table(&areas));
    if let Some(path) = &args.svg {
        write_svg(path, &areas)?;
    }
    Ok(())
}

fn write_svg(path: &Path, areas: &[Area]) -> Result<(), MeasureError> {
    write_text(path, &translation::svg(areas, translation::SVG_TITLE))
        .map_err(|e| MeasureError::Io(path.to_path_buf(), e))
}
