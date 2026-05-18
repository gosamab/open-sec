//! Public types returned by the store, plus small internal converters that
//! map between in-memory `scanner` enums and the TEXT representations we
//! persist. Kept here so each table module can stay focused on SQL.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriageStatus {
    Accepted,
    Dismissed,
    Snoozed,
}

impl TriageStatus {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            TriageStatus::Accepted => "accepted",
            TriageStatus::Dismissed => "dismissed",
            TriageStatus::Snoozed => "snoozed",
        }
    }
    pub fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "accepted" => TriageStatus::Accepted,
            "dismissed" => TriageStatus::Dismissed,
            "snoozed" => TriageStatus::Snoozed,
            other => return Err(anyhow!("unknown triage status: {other}")),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedPatchRecord {
    pub finding_id: String,
    pub file: String,
    pub applied_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriageRecord {
    pub finding_id: String,
    pub status: TriageStatus,
    pub reason: Option<String>,
    pub snooze_until: Option<i64>,
    pub updated_at: i64,
}

/// Summary row shown in the launcher (one per root, latest scan only).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanGroup {
    pub root: String,
    pub latest_scan_id: String,
    pub latest_started_at: i64,
    pub latest_kept: i64,
}

/// Raw row from the `findings` table. Kept here (rather than in `scans.rs`)
/// because both `save_scan` and `load_scan` map between this and
/// `crate::scanner::Finding`.
pub(super) struct StoredFinding {
    pub finding_id: String,
    pub rel_path: String,
    pub kind: String,
    pub severity: String,
    pub cwe: String,
    pub owasp: Option<String>,
    pub title: String,
    pub file: String,
    pub line_start: i64,
    pub line_end: i64,
    pub description: String,
    pub data_flow: String,
    pub verdict_json: Option<String>,
    pub patch_json: Option<String>,
}

pub(super) fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 24-hex-char id with a millis prefix so SQL ORDER BY id ≈ chronological.
pub(super) fn new_scan_id() -> String {
    let mut hasher = Sha256::new();
    let ts = now_ms();
    hasher.update(ts.to_le_bytes());
    hasher.update(rand_bytes());
    let digest = hasher.finalize();
    let mut s = String::with_capacity(24);
    use std::fmt::Write;
    for b in digest.iter().take(12) {
        let _ = write!(&mut s, "{:02x}", b);
    }
    s
}

fn rand_bytes() -> [u8; 16] {
    // Cheap entropy — combine pointer + nanos. Good enough for ID uniqueness;
    // we're not using these as security tokens.
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u128)
        .unwrap_or(0);
    let ptr = &nanos as *const _ as usize as u128;
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&nanos.to_le_bytes()[..8]);
    out[8..].copy_from_slice(&ptr.to_le_bytes()[..8]);
    out
}

pub(super) fn kind_str(k: crate::scanner::FindingKind) -> &'static str {
    match k {
        crate::scanner::FindingKind::Vuln => "vuln",
        crate::scanner::FindingKind::Hardening => "hardening",
    }
}

pub(super) fn kind_from_str(s: &str) -> Result<crate::scanner::FindingKind> {
    Ok(match s {
        "vuln" => crate::scanner::FindingKind::Vuln,
        "hardening" => crate::scanner::FindingKind::Hardening,
        other => return Err(anyhow!("unknown finding kind in DB: {other}")),
    })
}

pub(super) fn severity_str(s: crate::scanner::Severity) -> &'static str {
    use crate::scanner::Severity::*;
    match s {
        Critical => "critical",
        High => "high",
        Medium => "medium",
        Low => "low",
        Info => "info",
    }
}

pub(super) fn severity_from_str(s: &str) -> Result<crate::scanner::Severity> {
    use crate::scanner::Severity::*;
    Ok(match s {
        "critical" => Critical,
        "high" => High,
        "medium" => Medium,
        "low" => Low,
        "info" => Info,
        other => return Err(anyhow!("unknown severity in DB: {other}")),
    })
}
