#![allow(dead_code)] // wired up incrementally as Step 4+ land

pub mod detect;
pub mod ingest;
pub mod patch;
pub mod triage;
mod util;
pub mod verify;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingKind {
    Vuln,
    Hardening,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// Stable hash — survives re-scans so dismiss/accept/snooze decisions carry over.
    /// Computed server-side via `assign_id` after the model returns; defaults so we
    /// don't require the model to emit it.
    #[serde(default)]
    pub id: String,
    pub kind: FindingKind,
    pub severity: Severity,
    pub cwe: String,
    #[serde(default)]
    pub owasp: Option<String>,
    pub title: String,
    pub file: String,
    pub line_start: u32,
    pub line_end: u32,
    pub description: String,
    pub data_flow: String,
}

impl Finding {
    /// Compute the stable id from the fields that uniquely identify the finding.
    pub fn assign_id(&mut self) {
        self.id = stable_id(
            &self.file,
            self.line_start,
            self.line_end,
            &self.cwe,
            &self.title,
        );
    }
}

pub fn stable_id(file: &str, line_start: u32, line_end: u32, cwe: &str, title: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(file.as_bytes());
    hasher.update(b"|");
    hasher.update(line_start.to_le_bytes());
    hasher.update(b"|");
    hasher.update(line_end.to_le_bytes());
    hasher.update(b"|");
    hasher.update(cwe.as_bytes());
    hasher.update(b"|");
    hasher.update(normalize(title).as_bytes());
    let digest = hasher.finalize();
    hex16(&digest)
}

fn hex16(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(16);
    for b in bytes.iter().take(8) {
        use std::fmt::Write;
        let _ = write!(&mut s, "{:02x}", b);
    }
    s
}

/// Collapse whitespace so trivial wording changes don't churn the id.
fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_id_is_deterministic() {
        let a = stable_id("src/a.ts", 10, 20, "CWE-89", "SQL injection in foo");
        let b = stable_id("src/a.ts", 10, 20, "CWE-89", "SQL injection in foo");
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn stable_id_ignores_title_whitespace_and_case() {
        let a = stable_id("src/a.ts", 10, 20, "CWE-89", "SQL injection in foo");
        let b = stable_id("src/a.ts", 10, 20, "CWE-89", "  sql  INJECTION   IN foo  ");
        assert_eq!(a, b);
    }

    #[test]
    fn stable_id_changes_on_file_or_line() {
        let base = stable_id("src/a.ts", 10, 20, "CWE-89", "x");
        assert_ne!(base, stable_id("src/b.ts", 10, 20, "CWE-89", "x"));
        assert_ne!(base, stable_id("src/a.ts", 11, 20, "CWE-89", "x"));
        assert_ne!(base, stable_id("src/a.ts", 10, 21, "CWE-89", "x"));
        assert_ne!(base, stable_id("src/a.ts", 10, 20, "CWE-90", "x"));
    }
}
