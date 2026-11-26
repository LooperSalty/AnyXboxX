//! Test bench of a package folder (`scarlett.py --banc`, `emu/bench.py`): every PE32+ module,
//! its imports and the share the runtime resolves.
//!
//! The Python bench also loads each module in the Unicorn sandbox and runs its initialisation.
//! The sandbox is dropped (decision D2 of docs/architecture-rust.md), so only the static columns
//! are measured here; see `anyxboxx banc` for how the other columns are reported.

use std::path::Path;

use crate::analyse;
use crate::modules::{file_name, package_modules};
use crate::text::py_round;

/// One module: imported functions and the share resolved (emulated or provided by the package).
#[derive(Debug, Clone, PartialEq)]
pub struct BenchRow {
    pub name: String,
    pub imports: usize,
    pub coverage: f64,
    /// The analysis failed: the module counts 0 imports, like the Python row "analyse : …".
    pub error: Option<String>,
}

impl BenchRow {
    pub fn new(name: &str, imports: usize, coverage: f64) -> Self {
        Self {
            name: name.into(),
            imports,
            coverage,
            error: None,
        }
    }
}

/// `bench.run` without the sandbox: the static analysis of every module of `folder`.
pub fn rows(
    folder: &Path,
    extra_dirs: &[&Path],
    emulated: &dyn Fn(&str, &str) -> bool,
) -> Vec<BenchRow> {
    package_modules(folder)
        .iter()
        .map(|path| match analyse(path, extra_dirs, emulated) {
            Ok(report) => BenchRow::new(&file_name(path), report.total, report.coverage()),
            Err(error) => BenchRow {
                error: Some(error.to_string()),
                ..BenchRow::new(&file_name(path), 0, 0.0)
            },
        })
        .collect()
}

/// "Not available" in the load and init columns: nothing is run.
pub const NOT_MEASURED: &str = "n/d";
/// The footer's first half, where Python counts the modules whose initialisation succeeded.
pub const NO_SANDBOX: &str = "Chargement et initialisation : non disponibles en Rust (bac à sable \
     Unicorn abandonné, décision D2 de docs/architecture-rust.md)";

/// Imports resolved over all rows: `round(imports * coverage / 100)` summed, as `bench.table`.
pub fn resolved(rows: &[BenchRow]) -> (usize, usize) {
    let total = rows.iter().map(|r| r.imports).sum();
    let done = rows
        .iter()
        .map(|r| py_round(r.imports as f64 * r.coverage / 100.0) as usize)
        .sum();
    (done, total)
}

fn row_line(row: &BenchRow) -> String {
    let (load, detail) = match &row.error {
        // The analysis failed: Python does not load the module either.
        Some(error) => ("non", format!("analyse : {error}")),
        None => (NOT_MEASURED, String::new()),
    };
    format!(
        "| {} | {} | {:.0} % | {load} | {load} | {detail} |",
        row.name, row.imports, row.coverage
    )
}

/// `bench.table`: the same columns; "Chargé" and "Init." say `n/d` for the modules analysed,
/// and the footer says why instead of counting successful initialisations.
pub fn table(rows: &[BenchRow]) -> String {
    let mut lines = vec![
        "| Module | Imports | Résolus | Chargé | Init. | Détail |".to_string(),
        "|---|---|---|---|---|---|".to_string(),
    ];
    lines.extend(rows.iter().map(row_line));
    let (done, total) = resolved(rows);
    let share = if total == 0 {
        0.0
    } else {
        (100 * done) as f64 / total as f64
    };
    lines.push(format!(
        "\n{NO_SANDBOX} · imports résolus : {done}/{total} ({share:.0} %)"
    ));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_say_what_was_not_measured() {
        let failed = BenchRow {
            error: Some("not a PE".into()),
            ..BenchRow::new("bad.dll", 0, 0.0)
        };
        let text = table(&[BenchRow::new("a.dll", 3, 200.0 / 3.0), failed]);
        assert!(text.contains("| a.dll | 3 | 67 % | n/d | n/d |  |"));
        assert!(text.contains("| bad.dll | 0 | 0 % | non | non | analyse : not a PE |"));
        assert!(text.ends_with("D2 de docs/architecture-rust.md) · imports résolus : 2/3 (67 %)"));
        assert!(!text.contains("oui"));
    }

    #[test]
    fn an_empty_folder_resolves_nothing() {
        assert!(table(&[]).ends_with("imports résolus : 0/0 (0 %)"));
    }
}
