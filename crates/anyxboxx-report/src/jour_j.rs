//! First launch of a console executable (`tools/jour_j.py`): the static part of its report,
//! in French like the Python tool. Inventory of the executable (hash, headers, sections, TLS,
//! imports), then who provides each imported function: the package, AnyXboxX, TomBox, Windows
//! or nobody, in the order the launcher resolves them.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use victory_pe::ImageInfo;

use crate::modules::{exported_names, file_name, symbol_name};
use crate::text::{capitalize, spaced, thousands};
use crate::{ReportError, dll_name, is_system};

pub const PROVIDERS: [&str; 4] = ["paquet", "AnyXboxX", "TomBox", "Windows"];
pub const MISSING: &str = "manquant";
const WINDOWS: &str = "Windows";
const API_SETS: [&str; 2] = ["api-ms-", "ext-ms-"];
const MAX_LISTED: usize = 25;

/// The folders searched for each provider, in this order.
#[derive(Debug, Clone)]
pub struct Folders {
    pub package: PathBuf,
    pub runtime: PathBuf,
    pub tombox: PathBuf,
    pub system32: PathBuf,
}

impl Folders {
    fn of(&self, label: &str) -> &Path {
        match label {
            "paquet" => &self.package,
            "AnyXboxX" => &self.runtime,
            "TomBox" => &self.tombox,
            _ => &self.system32,
        }
    }
}

/// `provider()` of jour_j.py, with the exports of each DLL read once.
pub struct Resolver {
    folders: Folders,
    exports: RefCell<HashMap<PathBuf, BTreeSet<String>>>,
}

impl Resolver {
    pub fn new(folders: Folders) -> Self {
        Self {
            folders,
            exports: RefCell::new(HashMap::new()),
        }
    }

    fn exported(&self, path: &Path, function: &str) -> bool {
        self.exports
            .borrow_mut()
            .entry(path.to_path_buf())
            .or_insert_with(|| exported_names(path))
            .contains(function)
    }

    /// First provider of `dll!function`, or `manquant`.
    pub fn provider(&self, dll: &str, function: &str) -> &'static str {
        let name = dll_name(dll);
        for label in PROVIDERS {
            if label == WINDOWS && API_SETS.iter().any(|p| name.starts_with(p)) {
                return label; // API sets: resolved by Windows
            }
            let path = self.folders.of(label).join(&name);
            if path.is_file() && (function.starts_with('#') || self.exported(&path, function)) {
                return label;
            }
        }
        if is_system(&name) && !self.folders.system32.join(&name).is_file() {
            WINDOWS
        } else {
            MISSING
        }
    }
}

/// The executable as `inventory()` reads it: its imports and the lines of the inventory.
pub struct Inventory {
    pub imports: Vec<(String, String)>,
    pub lines: Vec<String>,
}

pub fn inventory(exe: &Path) -> Result<Inventory, ReportError> {
    let data = std::fs::read(exe)?;
    let info = ImageInfo::parse(&data).map_err(|e| ReportError::Package(e.to_string()))?;
    let imports: Vec<(String, String)> = info
        .imports
        .iter()
        .flat_map(|m| {
            m.entries
                .iter()
                .map(|e| (m.dll.clone(), symbol_name(&e.symbol)))
        })
        .collect();
    let dlls: BTreeSet<String> = imports.iter().map(|(d, _)| d.to_lowercase()).collect();
    let mut lines = vec![
        spaced(&format!(
            "- Fichier : `{}`, {} octets",
            file_name(exe),
            thousands(data.len() as u64)
        )),
        format!("- SHA-256 : `{:x}`", Sha256::digest(&data)),
        format!(
            "- Machine 0x{:04X}, sous-système {}, base 0x{:X}, point d'entrée 0x{:X}",
            info.machine, info.subsystem, info.image_base, info.entry_rva
        ),
        format!("- TLS : {}", if info.tls.is_some() { "oui" } else { "non" }),
        format!(
            "- Imports : {} fonctions dans {} DLL",
            imports.len(),
            dlls.len()
        ),
        String::new(),
        "| Section | Taille virtuelle | Taille brute |".into(),
        "|---|---:|---:|".into(),
    ];
    lines.extend(info.sections.iter().map(|s| {
        spaced(&format!(
            "| `{}` | {} | {} |",
            s.name.trim_end_matches('\0'),
            thousands(u64::from(s.virtual_size)),
            thousands(u64::from(s.file_size))
        ))
    }));
    Ok(Inventory { imports, lines })
}

/// Counts per provider label (the four providers, then `manquant`).
pub type Counts = BTreeMap<&'static str, usize>;

fn labels() -> impl Iterator<Item = &'static str> {
    PROVIDERS.into_iter().chain([MISSING])
}

fn counts_row(counts: &Counts) -> String {
    labels()
        .map(|l| counts.get(l).copied().unwrap_or(0).to_string())
        .collect::<Vec<_>>()
        .join(" | ")
}

/// `coverage()`: totals per provider and the lines of the table, then the missing functions.
pub fn coverage(imports: &[(String, String)], resolver: &Resolver) -> (Counts, Vec<String>) {
    let mut by_dll: Vec<(String, Counts)> = Vec::new();
    let mut missing: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for (dll, function) in imports {
        let found = resolver.provider(dll, function);
        let name = dll_name(dll);
        let index = match by_dll.iter().position(|(d, _)| *d == name) {
            Some(index) => index,
            None => {
                by_dll.push((name.clone(), Counts::new()));
                by_dll.len() - 1
            }
        };
        *by_dll[index].1.entry(found).or_default() += 1;
        if found == MISSING {
            missing.entry(name).or_default().push(function);
        }
    }
    // Most missing first; equal counts keep the order of the imports (a stable sort).
    by_dll.sort_by_key(|(_, c)| std::cmp::Reverse(c.get(MISSING).copied().unwrap_or(0)));
    let header: Vec<String> = labels().map(capitalize).collect();
    let mut lines = vec![
        format!("| DLL | Imports | {} |", header.join(" | ")),
        format!("|---|---:|{}", "---:|".repeat(header.len())),
    ];
    let mut totals = Counts::new();
    for (dll, counts) in &by_dll {
        for (label, count) in counts {
            *totals.entry(label).or_default() += count;
        }
        let sum: usize = counts.values().sum();
        lines.push(format!("| `{dll}` | {sum} | {} |", counts_row(counts)));
    }
    let total: usize = totals.values().sum();
    lines.push(format!("| **Total** | {total} | {} |", counts_row(&totals)));
    for (dll, functions) in &missing {
        let shown: Vec<String> = functions
            .iter()
            .take(MAX_LISTED)
            .map(|f| format!("`{f}`"))
            .collect();
        let more = if functions.len() > MAX_LISTED {
            format!(" … et {} autres", functions.len() - MAX_LISTED)
        } else {
            String::new()
        };
        lines.push(format!(
            "\n- `{dll}` manquantes : {}{more}",
            shown.join(", ")
        ));
    }
    (totals, lines)
}

/// The static part of the report: title, summary, inventory and providers, ending with a blank
/// line. The launch sections follow it.
pub fn static_report(exe: &Path, resolver: &Resolver) -> Result<Vec<String>, ReportError> {
    let inventory = inventory(exe)?;
    let (totals, coverage_lines) = coverage(&inventory.imports, resolver);
    let total: usize = totals.values().sum();
    let missing = totals.get(MISSING).copied().unwrap_or(0);
    let mut lines = vec![
        format!("# Premier lancement : {}", file_name(exe)),
        String::new(),
        format!(
            "Imports couverts : **{}/{total}** ; manquants : **{missing}**.",
            total - missing
        ),
        String::new(),
        "## Inventaire".into(),
        String::new(),
    ];
    lines.extend(inventory.lines);
    lines.extend([
        String::new(),
        "## Qui fournit chaque import".into(),
        String::new(),
    ]);
    lines.extend(coverage_lines);
    lines.push(String::new());
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver() -> Resolver {
        let nowhere = std::env::temp_dir().join("anyxboxx-jour-j-nowhere");
        Resolver::new(Folders {
            package: nowhere.clone(),
            runtime: nowhere.clone(),
            tombox: nowhere.clone(),
            system32: nowhere,
        })
    }

    #[test]
    fn system_dlls_and_api_sets_fall_to_windows() {
        let resolver = resolver();
        assert_eq!(resolver.provider("KERNEL32.dll", "Sleep"), "Windows");
        assert_eq!(
            resolver.provider("api-ms-win-core-synch-l1-2-0", "F"),
            "Windows"
        );
        assert_eq!(resolver.provider("absent.dll", "#3"), MISSING);
    }

    #[test]
    fn coverage_sorts_by_missing_and_lists_them() {
        let imports: Vec<(String, String)> = [
            ("KERNEL32.dll", "Sleep"),
            ("absent.dll", "#3"),
            ("kernel32.dll", "Exit"),
            ("other.dll", "A"),
            ("absent.dll", "B"),
        ]
        .iter()
        .map(|(d, f)| (d.to_string(), f.to_string()))
        .collect();
        let (totals, lines) = coverage(&imports, &resolver());
        assert_eq!(totals[MISSING], 3);
        assert_eq!(
            lines[0],
            "| DLL | Imports | Paquet | Anyxboxx | Tombox | Windows | Manquant |"
        );
        assert_eq!(lines[2], "| `absent.dll` | 2 | 0 | 0 | 0 | 0 | 2 |");
        assert_eq!(lines[3], "| `other.dll` | 1 | 0 | 0 | 0 | 0 | 1 |");
        assert_eq!(lines[4], "| `kernel32.dll` | 2 | 0 | 0 | 0 | 2 | 0 |");
        assert_eq!(lines[5], "| **Total** | 5 | 0 | 0 | 0 | 2 | 3 |");
        assert_eq!(lines[6], "\n- `absent.dll` manquantes : `#3`, `B`");
    }
}
