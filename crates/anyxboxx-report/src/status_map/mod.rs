//! Progress map (`scarlett.py --carte`, `emu/status_map.py`): SVG treemaps, one cell per
//! function, green when translated, grey otherwise, and the badges printed under it.

use std::path::Path;

use crate::analyse;
use crate::gdk::GdkRuntime;
use crate::gpu::{GPU_FAMILIES, GpuCoverage};
use crate::modules::{file_name, package_modules};
use crate::translation::measure;

mod render;
mod treemap;

pub use render::{badge_color, render};
pub use treemap::{Rect, squarify};

const GPU_TITLE: &str = "RDNA 2 GPU instructions (reference interpreter)";
const DISASSEMBLY_TITLE: &str = "RDNA 2 GPU instructions (disassembler, LLVM syntax)";

/// A treemap rectangle: `done` functions translated out of `total`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub name: String,
    pub done: usize,
    pub total: usize,
}

impl Block {
    pub fn new(name: &str, done: usize, total: usize) -> Self {
        Self {
            name: name.into(),
            done,
            total,
        }
    }
}

/// A treemap. Not `counted`: blocks without cells or numbers, only a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub blocks: Vec<Block>,
    pub counted: bool,
    pub note: String,
}

impl Section {
    pub fn counted(title: &str, blocks: Vec<Block>) -> Self {
        Self {
            title: title.into(),
            blocks,
            counted: true,
            note: String::new(),
        }
    }

    pub fn done(&self) -> usize {
        self.blocks.iter().map(|b| b.done).sum()
    }

    pub fn total(&self) -> usize {
        self.blocks.iter().map(|b| b.total).sum()
    }

    /// Shown title: "GDK runtime: 17.9% (71/396)".
    pub fn heading(&self) -> String {
        if !self.counted {
            return format!("{}: 0% ({})", self.title, self.note);
        }
        let share = match self.total() {
            0 => 0.0,
            total => (100 * self.done()) as f64 / total as f64,
        };
        format!(
            "{}: {share:.1}% ({}/{})",
            self.title,
            self.done(),
            self.total()
        )
    }
}

/// A header badge: label, value, share from 0 to 1.
#[derive(Debug, Clone, PartialEq)]
pub struct Badge {
    pub label: String,
    pub value: String,
    pub share: f64,
}

impl Badge {
    fn count(label: &str, done: usize, total: usize) -> Self {
        Self {
            label: label.into(),
            value: format!("{done}/{total}"),
            share: if total == 0 {
                0.0
            } else {
                done as f64 / total as f64
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub sections: Vec<Section>,
    pub badges: Vec<Badge>,
}

/// `status_map.package_blocks`: one block per module of the package, imports resolved out of
/// imports in total; modules that cannot be analysed or import nothing are left out.
pub fn package_blocks(package: &Path, emulated: &dyn Fn(&str, &str) -> bool) -> Vec<Block> {
    package_modules(package)
        .iter()
        .filter_map(|path| {
            let report = analyse(path, &[], emulated).ok()?;
            let done = report.emulated.len() + report.provided.len();
            (report.total > 0).then(|| Block::new(&file_name(path), done, report.total))
        })
        .collect()
}

/// One block per RDNA 2 family of the map, from the GPU table.
fn gpu_blocks(
    gpu: &GpuCoverage,
    done: impl Fn(&crate::gpu::FamilyCoverage) -> usize,
) -> Vec<Block> {
    GPU_FAMILIES
        .iter()
        .filter_map(|family| gpu.family(family))
        .filter(|f| f.total > 0)
        .map(|f| Block::new(&f.family, done(f), f.total))
        .collect()
}

/// The inputs of the map.
pub struct MapInputs<'a> {
    pub package: &'a Path,
    pub runtime: &'a Path,
    pub tombox: &'a Path,
    pub gdk: &'a GdkRuntime,
    pub gpu: &'a GpuCoverage,
    pub emulated: &'a dyn Fn(&str, &str) -> bool,
}

/// `status_map.collect`: the sections of the map and the header badges.
pub fn collect(inputs: &MapInputs<'_>) -> Status {
    let gdk_blocks = inputs
        .gdk
        .blocks()
        .into_iter()
        .map(|(name, done, total)| Block::new(&name, done, total))
        .collect();
    let gdk = Section::counted("GDK runtime", gdk_blocks);
    let imports = Section::counted(
        "Package imports",
        package_blocks(inputs.package, inputs.emulated),
    );
    let console_blocks = measure(inputs.package, Some(inputs.runtime), Some(inputs.tombox))
        .into_iter()
        .map(|a| Block::new(&a.name, a.done, a.total))
        .collect();
    let console = Section::counted("Console DLLs", console_blocks);
    let disassembly = Section::counted(
        DISASSEMBLY_TITLE,
        gpu_blocks(inputs.gpu, |f| f.disassembled),
    );
    let gpu = Section::counted(GPU_TITLE, gpu_blocks(inputs.gpu, |f| f.executed));
    let badges = vec![
        Badge {
            label: "CPU x86-64".into(),
            value: "100%".into(),
            share: 1.0,
        },
        Badge::count("GDK runtime", gdk.done(), gdk.total()),
        Badge::count("Package imports", imports.done(), imports.total()),
        Badge::count("Console DLLs", console.done(), console.total()),
        Badge::count("GPU disassembly", disassembly.done(), disassembly.total()),
        Badge::count("GPU RDNA 2", gpu.done(), gpu.total()),
    ];
    Status {
        sections: vec![gdk, imports, console, disassembly, gpu],
        badges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> Status {
        Status {
            sections: vec![
                Section::counted(
                    "GDK runtime",
                    vec![Block::new("XTaskQueue", 3, 5), Block::new("XUser", 0, 4)],
                ),
                Section::counted("DLL <console>", vec![Block::new("xmem.dll", 1, 2)]),
                Section {
                    title: "GPU".into(),
                    blocks: vec![Block::new("SOP1", 0, 0), Block::new("VOP3", 0, 0)],
                    counted: false,
                    note: "recompiler not started".into(),
                },
            ],
            badges: vec![Badge::count("Package imports", 1, 4)],
        }
    }

    #[test]
    fn one_green_cell_per_translated_function() {
        let image = render(&status());
        assert_eq!(image.matches(r#"class="ok""#).count(), 4);
        assert_eq!(image.matches(r#"class="no""#).count(), (5 - 3) + 4 + 1);
        assert!(image.contains("DLL &lt;console&gt;"));
        assert!(image.contains("<title>XTaskQueue: 3/5</title>"));
        assert!(image.contains("GDK runtime: 33.3% (3/9)"));
        assert!(image.contains("GPU: 0% (recompiler not started)"));
    }

    #[test]
    fn empty_sections_are_left_out() {
        let empty = Status {
            sections: vec![Section::counted("Nothing", vec![Block::new("a", 0, 0)])],
            badges: Vec::new(),
        };
        assert!(!render(&empty).contains("Nothing"));
    }
}
