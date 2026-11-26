//! Translation progress of the Xbox Series X console DLLs (`scarlett.py --traduction`,
//! `emu/translation.py`): one `Area` per domain, a Markdown table and an SVG bar chart.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::bench::{BenchRow, resolved};
use crate::gdk::GdkRuntime;
use crate::gpu::GpuCoverage;
use crate::modules::{exported_names, imported_names, package_modules};
use crate::text::{escape, py_round};

pub const CONSOLE_DLLS: [&str; 9] = [
    "xmem.dll",
    "d3d12_xs.dll",
    "kernelx.dll",
    "xg_xs.dll",
    "xgameruntime.dll",
    "xcrdapi.dll",
    "xapu.dll",
    "acphal.dll",
    "chad.dll",
];
/// `tombox_paths.TOMBOX_PREFERRED`: TomBox DLLs the launcher loads instead of ours.
pub const TOMBOX_PREFERRED: [&str; 3] = ["d3d12_xs.dll", "xmem.dll", "xcrdapi.dll"];
/// Exported by the runtime but only started: they report a clean failure.
const PARTIAL: [(&str, &str); 1] = [("XMemGetWorkingSetStatistics", "non-public structure")];
const COMPLETE: &str = "complete";
const MAX_LISTED: usize = 4;
const WIDTH: i64 = 760;
const MARGIN: i64 = 24;
const LABEL_W: i64 = 270;
const BAR_X: i64 = 300;
const BAR_W: i64 = 220;
const HEADER_H: i64 = 76;
const ROW_H: i64 = 46;
const GREEN: &str = "#107c10";
const ORANGE: &str = "#e8890c";
const RED: &str = "#c42b1c";
const TRACK: &str = "#e3e6ee";
pub const SVG_TITLE: &str = "AnyXboxX: Xbox Series X translation progress";

/// A domain to translate: `done` items out of `total`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Area {
    pub name: String,
    pub done: usize,
    pub total: usize,
    pub note: String,
    /// False: only the share is shown (a domain that cannot be counted).
    pub counted: bool,
}

impl Area {
    pub fn new(name: &str, done: usize, total: usize, note: &str) -> Self {
        Self {
            name: name.into(),
            done,
            total,
            note: note.into(),
            counted: true,
        }
    }

    fn uncounted(name: &str, note: &str) -> Self {
        Self {
            counted: false,
            ..Self::new(name, 1, 1, note)
        }
    }

    /// Share translated, 0 to 1 (0 when there is nothing to translate).
    pub fn share(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.done as f64 / self.total as f64
        }
    }
}

pub fn cpu_area() -> Area {
    Area::uncounted(
        "CPU instructions (x86-64, Zen 2)",
        "same instruction set as the PC: run as-is in native mode",
    )
}

pub fn static_areas() -> Vec<Area> {
    vec![Area::uncounted(
        "D3D12.X: device and ID3D12* interfaces",
        "served by TomBox's d3d12_xs.dll (194/200 methods by their count), not yet tried with a game",
    )]
}

pub fn gpu_area(gpu: &GpuCoverage) -> Area {
    let (done, total) = gpu.executed();
    Area::new(
        "GPU instructions (RDNA 2, console shaders)",
        done,
        total,
        "run by the reference interpreter (wave32); recompiled to DXIL: compute, vertex, pixel",
    )
}

pub fn gpu_disassembly_area(gpu: &GpuCoverage) -> Area {
    let (done, total) = gpu.disassembled();
    Area::new(
        "GPU instructions disassembled (RDNA 2, LLVM syntax)",
        done,
        total,
        "operands checked against LLVM's public encoding tests, SDWA and DPP forms included",
    )
}

pub fn gdk_runtime_area(gdk: &GdkRuntime) -> Area {
    let (served, interfaces) = gdk.interfaces();
    Area::new(
        "GDK runtime functions served by AnyXboxX",
        gdk.done(),
        gdk.apis.len(),
        &format!("{served}/{interfaces} interfaces (used when the Windows GDK runtime is off)"),
    )
}

/// System functions imported by the package and resolved (emulated or provided by a DLL).
pub fn functions_area(rows: &[BenchRow]) -> Area {
    let (done, total) = resolved(rows);
    Area::new(
        "System functions imported by the package",
        done,
        total,
        "isolated mode: emulated or provided by a package DLL",
    )
}

type Names = BTreeMap<String, BTreeSet<String>>;

/// `translation.package_imports`: functions the package imports, by console DLL.
pub fn package_imports(folder: &Path) -> Names {
    let mut merged = Names::new();
    for path in package_modules(folder) {
        for (dll, names) in imported_names(&path) {
            if CONSOLE_DLLS.contains(&dll.as_str()) {
                merged.entry(dll).or_default().extend(names);
            }
        }
    }
    merged
}

/// Named exports of the `names` DLLs present in `folder` (none when the folder is missing).
fn exports_in(folder: Option<&Path>, names: &[&str]) -> Names {
    let Some(folder) = folder.filter(|f| f.is_dir()) else {
        return Names::new();
    };
    names
        .iter()
        .filter_map(|name| {
            let path = folder.join(name);
            path.is_file()
                .then(|| (name.to_string(), exported_names(&path)))
        })
        .collect()
}

fn partial_reason(name: &str) -> Option<&'static str> {
    PARTIAL
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, why)| *why)
}

/// `translation.measure`: one area per console DLL the package imports. The runtime's exports
/// count, and so do the TomBox DLLs the launcher prefers.
pub fn measure(package: &Path, runtime: Option<&Path>, tombox: Option<&Path>) -> Vec<Area> {
    let imports = package_imports(package);
    let exports = exports_in(runtime, &CONSOLE_DLLS);
    let tombox = exports_in(tombox, &TOMBOX_PREFERRED);
    let empty = BTreeSet::new();
    CONSOLE_DLLS
        .iter()
        .filter_map(|dll| {
            let wanted = imports.get(*dll)?;
            let from_tombox = tombox.get(*dll).unwrap_or(&empty);
            let exported: BTreeSet<&String> = exports
                .get(*dll)
                .unwrap_or(&empty)
                .union(from_tombox)
                .collect();
            let (found, missing): (Vec<&String>, Vec<&String>) =
                wanted.iter().partition(|n| exported.contains(n));
            let mut remaining: Vec<String> = missing.into_iter().cloned().collect();
            remaining.extend(found.into_iter().filter_map(|name| {
                let why = partial_reason(name).filter(|_| !from_tombox.contains(name))?;
                Some(format!("{name} (partial: {why})"))
            }));
            Some(Area::new(
                dll,
                wanted.len() - remaining.len(),
                wanted.len(),
                &note(&remaining),
            ))
        })
        .collect()
}

fn note(remaining: &[String]) -> String {
    if remaining.is_empty() {
        return COMPLETE.into();
    }
    let listed = remaining[..remaining.len().min(MAX_LISTED)].join(", ");
    if remaining.len() > MAX_LISTED {
        listed + ", …"
    } else {
        listed
    }
}

/// Markdown table of the progress.
pub fn table(areas: &[Area]) -> String {
    let mut lines = vec![
        "| Domaine | Traduit | Total | Part | Reste |".to_string(),
        "|---|---|---|---|---|".to_string(),
    ];
    lines.extend(areas.iter().map(|a| {
        format!(
            "| {} | {} | {} | {:.0} % | {} |",
            a.name,
            a.done,
            a.total,
            a.share() * 100.0,
            a.note
        )
    }));
    lines.join("\n")
}

fn color(area: &Area) -> &'static str {
    if area.total > 0 && area.done >= area.total {
        GREEN
    } else if area.done > 0 {
        ORANGE
    } else {
        RED
    }
}

fn amount(area: &Area) -> String {
    let share = format!("{:.0}%", area.share() * 100.0);
    if area.counted {
        format!("{}/{} ({share})", area.done, area.total)
    } else {
        share
    }
}

fn row(area: &Area, top: i64) -> [String; 5] {
    let mid = top + 14;
    let fill = py_round(BAR_W as f64 * area.share());
    [
        format!(
            r##"<text x="{LABEL_W}" y="{mid}" text-anchor="end" fill="#1d2230">{}</text>"##,
            escape(&area.name)
        ),
        format!(
            r#"<rect x="{BAR_X}" y="{top}" width="{BAR_W}" height="18" rx="3" fill="{TRACK}"/>"#
        ),
        format!(
            r#"<rect x="{BAR_X}" y="{top}" width="{fill}" height="18" rx="3" fill="{}"/>"#,
            color(area)
        ),
        format!(
            r##"<text x="{}" y="{mid}" fill="#1d2230">{}</text>"##,
            BAR_X + BAR_W + 12,
            amount(area)
        ),
        format!(
            r##"<text x="{BAR_X}" y="{}" font-size="11" fill="#4b5263">{}</text>"##,
            top + 34,
            escape(&area.note)
        ),
    ]
}

/// Standalone SVG image of the progress, one bar per domain.
pub fn svg(areas: &[Area], title: &str) -> String {
    let height = HEADER_H + ROW_H * areas.len() as i64 + 12;
    let mut body = vec![
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{height}" viewBox="0 0 {WIDTH} {height}" font-family="Segoe UI, Arial, sans-serif" font-size="13">"#
        ),
        format!(r##"<rect width="{WIDTH}" height="{height}" fill="#f7f8fa"/>"##),
        format!(
            r##"<text x="{MARGIN}" y="34" font-size="18" font-weight="600" fill="#1d2230">{}</text>"##,
            escape(title)
        ),
        format!(
            r##"<text x="{MARGIN}" y="56" fill="#4b5263">Share translated for the Xbox Series X · green complete, orange partial, red not started</text>"##
        ),
    ];
    for (index, area) in areas.iter().enumerate() {
        body.extend(row(area, HEADER_H + ROW_H * index as i64));
    }
    body.push("</svg>".into());
    body.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_list_four_then_an_ellipsis() {
        let names: Vec<String> = (1..=5).map(|n| format!("F{n}")).collect();
        assert_eq!(note(&names[..4]), "F1, F2, F3, F4");
        assert_eq!(note(&names), "F1, F2, F3, F4, …");
        assert_eq!(note(&[]), "complete");
    }

    #[test]
    fn table_rounds_shares_like_python() {
        let text = table(&[Area::new("x", 1, 8, "n"), Area::new("y", 0, 0, "m")]);
        assert!(text.contains("| x | 1 | 8 | 12 % | n |")); // 12.5 rounds to even
        assert!(text.contains("| y | 0 | 0 | 0 % | m |"));
    }

    #[test]
    fn uncounted_areas_show_a_share_only() {
        let image = svg(&[cpu_area()], "t <&>");
        assert!(image.contains(">100%</text>") && !image.contains("1/1"));
        assert!(image.contains("t &lt;&amp;&gt;"));
    }

    #[test]
    fn functions_area_rounds_each_row() {
        let rows = [
            BenchRow::new("a.dll", 100, 50.0),
            BenchRow::new("b.dll", 20, 100.0),
        ];
        let area = functions_area(&rows);
        assert_eq!((area.done, area.total), (70, 120));
    }
}
