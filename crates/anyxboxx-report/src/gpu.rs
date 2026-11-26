//! RDNA 2 coverage per encoding family: how many (family, opcode) encodings the disassembler
//! writes with their operands and how many the reference interpreter executes.
//!
//! Both are measured by Python code that is not ported yet (`emu/rdna2_coverage.py`,
//! `emu/rdna2_exec.py`, phase 4 of docs/architecture-rust.md). Until then the numbers come from a
//! table: `data/rdna2-coverage.txt`, built into the binary, which
//! `tests/test_rust_traduction_parity.py` compares with the Python measure (it fails when the
//! table is out of date and prints the line to update it). `--gpu-coverage <file>` reads
//! another table in the same format.

use std::path::Path;

/// The snapshot measured from the Python oracle.
pub const SNAPSHOT: &str = include_str!("../data/rdna2-coverage.txt");

/// `status_map.GPU_FAMILIES`: the families shown on the map, in this order.
pub const GPU_FAMILIES: [&str; 20] = [
    "SOP1", "SOP2", "SOPK", "SOPC", "SOPP", "SMEM", "VOP1", "VOP2", "VOP3", "VOP3P", "VOPC", "DS",
    "MUBUF", "MTBUF", "MIMG", "FLAT", "GLOBAL", "SCRATCH", "EXP", "VINTRP",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FamilyCoverage {
    pub family: String,
    pub disassembled: usize,
    pub executed: usize,
    pub total: usize,
}

/// The table: one family per line, `FAMILY disassembled executed total`; `#` starts a comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuCoverage {
    pub families: Vec<FamilyCoverage>,
}

impl GpuCoverage {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut families: Vec<FamilyCoverage> = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split_whitespace().collect();
            let numbers: Vec<usize> = fields[1..].iter().map_while(|n| n.parse().ok()).collect();
            let (4, &[disassembled, executed, total]) = (fields.len(), numbers.as_slice()) else {
                return Err(format!(
                    "GPU coverage table, line {}: expected `FAMILY disassembled executed total`",
                    index + 1
                ));
            };
            if disassembled > total || executed > total {
                return Err(format!(
                    "GPU coverage table, line {}: count above total",
                    index + 1
                ));
            }
            if families.iter().any(|f| f.family == fields[0]) {
                return Err(format!("GPU coverage table: {} listed twice", fields[0]));
            }
            families.push(FamilyCoverage {
                family: fields[0].to_string(),
                disassembled,
                executed,
                total,
            });
        }
        Ok(Self { families })
    }

    /// The table of `--gpu-coverage`, or the built-in snapshot.
    pub fn load(path: Option<&Path>) -> Result<Self, String> {
        match path {
            Some(path) => {
                let text = std::fs::read_to_string(path)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                Self::parse(&text)
            }
            None => Self::parse(SNAPSHOT),
        }
    }

    pub fn family(&self, name: &str) -> Option<&FamilyCoverage> {
        self.families.iter().find(|f| f.family == name)
    }

    /// `rdna2_coverage.coverage()`: (encodings disassembled, encodings known).
    pub fn disassembled(&self) -> (usize, usize) {
        (
            self.families.iter().map(|f| f.disassembled).sum(),
            self.total(),
        )
    }

    /// `rdna2_exec.coverage()`: (encodings executed, encodings known).
    pub fn executed(&self) -> (usize, usize) {
        (self.families.iter().map(|f| f.executed).sum(), self.total())
    }

    fn total(&self) -> usize {
        self.families.iter().map(|f| f.total).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_snapshot_parses_and_covers_known_families_only() {
        let table = GpuCoverage::parse(SNAPSHOT).unwrap();
        assert!(!table.families.is_empty());
        assert!(
            table
                .families
                .iter()
                .all(|f| GPU_FAMILIES.contains(&f.family.as_str()))
        );
        let (done, total) = table.executed();
        assert!(0 < done && done < total);
    }

    #[test]
    fn malformed_tables_are_refused() {
        assert!(GpuCoverage::parse("SOP1 1 2").is_err());
        assert!(GpuCoverage::parse("SOP1 1 2 x").is_err());
        assert!(GpuCoverage::parse("SOP1 3 1 2").is_err());
        assert!(GpuCoverage::parse("SOP1 1 1 2\nSOP1 1 1 2").is_err());
        let table = GpuCoverage::parse("# comment\n\nSOP1 1 0 2 # tail\nDS 2 1 2").unwrap();
        assert_eq!(table.disassembled(), (3, 4));
        assert_eq!(table.executed(), (1, 4));
        assert_eq!(table.family("DS").unwrap().total, 2);
    }
}
