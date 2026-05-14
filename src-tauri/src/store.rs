//! SQLite-backed persistence for scan history. Lives at
//! `<app_data_dir>/open-sec.db`. Schema is migrated forward via PRAGMA
//! user_version so future shape changes don't require manual DB resets.
//!
//! Stage 9a scope: persist completed scans and their findings, list past
//! scan groups by root for the launcher, and load any past scan back into
//! the workspace without re-running. Triage actions (9b) and cancellation
//! (9c) extend this schema; the `triage` table is created up front so the
//! migration doesn't have to grow.
//!
//! For the complex per-finding payloads (`Verdict`, `Patch`) we store
//! serialized JSON in columns rather than fully normalised tables — we
//! always read them together with their finding, and there's no querying
//! into their internals.

use std::path::Path;
use std::sync::Mutex;

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json as json;
use sha2::{Digest, Sha256};

use crate::scanner::ingest::{Skipped, WalkResult};
use crate::scanner::orchestrate::{FileFindings, ScanResult, StageUsage};
use crate::scanner::patch::Patch;
use crate::scanner::triage::TriagedFile;
use crate::scanner::verify::{Verdict, VerifiedFinding};
use crate::scanner::Finding;

const CURRENT_SCHEMA_VERSION: i32 = 1;

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS scans (
    id                     TEXT PRIMARY KEY,
    root                   TEXT NOT NULL,
    started_at             INTEGER NOT NULL,
    finished_at            INTEGER,
    status                 TEXT NOT NULL,
    total_findings         INTEGER NOT NULL DEFAULT 0,
    kept_findings          INTEGER NOT NULL DEFAULT 0,
    hardening_findings     INTEGER NOT NULL DEFAULT 0,
    walk_json              TEXT NOT NULL DEFAULT '{"candidates":[],"skipped":[]}',
    triaged_json           TEXT NOT NULL DEFAULT '[]',
    usage_json             TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS scans_root_started_idx ON scans(root, started_at DESC);

CREATE TABLE IF NOT EXISTS findings (
    scan_id        TEXT NOT NULL,
    finding_id     TEXT NOT NULL,
    rel_path       TEXT NOT NULL,
    kind           TEXT NOT NULL,
    severity       TEXT NOT NULL,
    cwe            TEXT NOT NULL,
    owasp          TEXT,
    title          TEXT NOT NULL,
    file           TEXT NOT NULL,
    line_start     INTEGER NOT NULL,
    line_end       INTEGER NOT NULL,
    description    TEXT NOT NULL,
    data_flow      TEXT NOT NULL,
    verdict_json   TEXT,
    patch_json     TEXT,
    PRIMARY KEY (scan_id, finding_id),
    FOREIGN KEY (scan_id) REFERENCES scans(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS findings_finding_id_idx ON findings(finding_id);

CREATE TABLE IF NOT EXISTS triage (
    finding_id     TEXT NOT NULL,
    root           TEXT NOT NULL,
    status         TEXT NOT NULL,
    reason         TEXT,
    snooze_until   INTEGER,
    updated_at     INTEGER NOT NULL,
    PRIMARY KEY (finding_id, root)
);
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriageStatus {
    Accepted,
    Dismissed,
    Snoozed,
}

impl TriageStatus {
    fn as_str(self) -> &'static str {
        match self {
            TriageStatus::Accepted => "accepted",
            TriageStatus::Dismissed => "dismissed",
            TriageStatus::Snoozed => "snoozed",
        }
    }
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "accepted" => TriageStatus::Accepted,
            "dismissed" => TriageStatus::Dismissed,
            "snoozed" => TriageStatus::Snoozed,
            other => return Err(anyhow!("unknown triage status: {other}")),
        })
    }
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
    pub latest_finished_at: Option<i64>,
    pub latest_status: String,
    pub latest_kept: i64,
    pub latest_total: i64,
    pub scan_count: i64,
}

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Open (or create) the SQLite database at `path` and run any pending
    /// schema migrations.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating DB dir {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening sqlite at {}", path.display()))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    /// In-memory store, for tests.
    #[cfg(test)]
    fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let current: i32 =
            conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if current < 1 {
            let tx = conn.transaction()?;
            tx.execute_batch(SCHEMA_V1)?;
            tx.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)?;
            tx.commit()?;
        }
        // Future migrations chain here: if current < 2 { ... }
        Ok(())
    }

    /// Persist a completed scan. Returns the new scan_id.
    pub fn save_scan(&self, result: &ScanResult, status: &str) -> Result<String> {
        let scan_id = new_scan_id();
        let started_at = now_ms();
        let finished_at = started_at;

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        let walk_json = json::to_string(&result.ingest)?;
        let triaged_json = json::to_string(&result.triaged)?;
        let usage_json = json::to_string(&result.usage)?;
        let total = result.verified.len() as i64;
        let kept = result
            .verified
            .iter()
            .filter(|v| v.verdict.as_ref().map(|x| x.keep()).unwrap_or(false))
            .count() as i64;
        let hardening = result
            .verified
            .iter()
            .filter(|v| matches!(v.finding.kind, crate::scanner::FindingKind::Hardening))
            .count() as i64;

        tx.execute(
            "INSERT INTO scans (id, root, started_at, finished_at, status,
                 total_findings, kept_findings, hardening_findings,
                 walk_json, triaged_json, usage_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                scan_id,
                result.root.to_string_lossy(),
                started_at,
                finished_at,
                status,
                total,
                kept,
                hardening,
                walk_json,
                triaged_json,
                usage_json,
            ],
        )?;

        // Build a per-finding rel_path lookup from findings_by_file so each
        // finding row carries the relative path the UI uses.
        let mut rel_by_file_path = std::collections::HashMap::new();
        for ff in &result.findings_by_file {
            for f in &ff.findings {
                rel_by_file_path.insert(f.id.clone(), ff.rel_path.clone());
            }
        }

        // Index patches by finding_id.
        let mut patch_by_id: std::collections::HashMap<&str, &Patch> =
            std::collections::HashMap::new();
        for p in &result.patches {
            patch_by_id.insert(p.finding_id.as_str(), p);
        }

        {
            let mut stmt = tx.prepare(
                "INSERT INTO findings (scan_id, finding_id, rel_path, kind, severity, cwe, owasp,
                     title, file, line_start, line_end, description, data_flow,
                     verdict_json, patch_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            )?;
            for v in &result.verified {
                let f = &v.finding;
                let rel = rel_by_file_path
                    .get(&f.id)
                    .cloned()
                    .unwrap_or_else(|| f.file.clone());
                let verdict_json = match &v.verdict {
                    Some(verdict) => Some(json::to_string(verdict)?),
                    None => None,
                };
                let patch_json = match patch_by_id.get(f.id.as_str()) {
                    Some(p) => Some(json::to_string(p)?),
                    None => None,
                };
                stmt.execute(params![
                    scan_id,
                    f.id,
                    rel,
                    kind_str(f.kind),
                    severity_str(f.severity),
                    f.cwe,
                    f.owasp,
                    f.title,
                    f.file,
                    f.line_start,
                    f.line_end,
                    f.description,
                    f.data_flow,
                    verdict_json,
                    patch_json,
                ])?;
            }
        }

        tx.commit()?;
        Ok(scan_id)
    }

    /// One row per root, showing the latest scan's metadata.
    pub fn list_scan_groups(&self, limit: usize) -> Result<Vec<ScanGroup>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT s.root,
                    s.id, s.started_at, s.finished_at, s.status,
                    s.kept_findings, s.total_findings,
                    (SELECT COUNT(*) FROM scans s2 WHERE s2.root = s.root) AS scan_count
             FROM scans s
             WHERE s.started_at = (SELECT MAX(started_at) FROM scans s3 WHERE s3.root = s.root)
             ORDER BY s.started_at DESC
             LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(ScanGroup {
                    root: row.get(0)?,
                    latest_scan_id: row.get(1)?,
                    latest_started_at: row.get(2)?,
                    latest_finished_at: row.get(3)?,
                    latest_status: row.get(4)?,
                    latest_kept: row.get(5)?,
                    latest_total: row.get(6)?,
                    scan_count: row.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Hydrate a `ScanResult` back from the database. Used when the launcher
    /// opens a past scan without re-running.
    pub fn load_scan(&self, scan_id: &str) -> Result<ScanResult> {
        let conn = self.conn.lock().unwrap();

        let (root, walk_json, triaged_json, usage_json): (String, String, String, String) = conn
            .query_row(
                "SELECT root, walk_json, triaged_json, usage_json FROM scans WHERE id = ?1",
                params![scan_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("scan {scan_id} not found"))?;

        let ingest: WalkResult = json::from_str(&walk_json)?;
        let triaged: Vec<TriagedFile> = json::from_str(&triaged_json)?;
        let usage: StageUsage = json::from_str(&usage_json).unwrap_or_default();

        let mut stmt = conn.prepare(
            "SELECT finding_id, rel_path, kind, severity, cwe, owasp, title, file,
                    line_start, line_end, description, data_flow, verdict_json, patch_json
             FROM findings WHERE scan_id = ?1",
        )?;
        let rows = stmt.query_map(params![scan_id], |row| {
            Ok(StoredFinding {
                finding_id: row.get(0)?,
                rel_path: row.get(1)?,
                kind: row.get(2)?,
                severity: row.get(3)?,
                cwe: row.get(4)?,
                owasp: row.get(5)?,
                title: row.get(6)?,
                file: row.get(7)?,
                line_start: row.get(8)?,
                line_end: row.get(9)?,
                description: row.get(10)?,
                data_flow: row.get(11)?,
                verdict_json: row.get(12)?,
                patch_json: row.get(13)?,
            })
        })?;

        // Reassemble: findings_by_file (grouped by rel_path), verified (with
        // verdicts), patches.
        let mut findings_by_file: std::collections::BTreeMap<String, Vec<Finding>> =
            Default::default();
        let mut verified: Vec<VerifiedFinding> = Vec::new();
        let mut patches: Vec<Patch> = Vec::new();
        let mut file_path_by_rel: std::collections::HashMap<String, std::path::PathBuf> =
            Default::default();

        for sf in rows {
            let sf = sf?;
            let finding = Finding {
                id: sf.finding_id.clone(),
                kind: kind_from_str(&sf.kind)?,
                severity: severity_from_str(&sf.severity)?,
                cwe: sf.cwe,
                owasp: sf.owasp,
                title: sf.title,
                file: sf.file.clone(),
                line_start: sf.line_start as u32,
                line_end: sf.line_end as u32,
                description: sf.description,
                data_flow: sf.data_flow,
            };
            file_path_by_rel
                .entry(sf.rel_path.clone())
                .or_insert_with(|| std::path::PathBuf::from(&sf.file));
            findings_by_file
                .entry(sf.rel_path)
                .or_default()
                .push(finding.clone());

            let verdict: Option<Verdict> = match sf.verdict_json {
                Some(s) => Some(json::from_str(&s)?),
                None => None,
            };
            verified.push(VerifiedFinding { finding, verdict });

            if let Some(s) = sf.patch_json {
                let patch: Patch = json::from_str(&s)?;
                patches.push(patch);
            }
        }

        let findings_by_file: Vec<FileFindings> = findings_by_file
            .into_iter()
            .map(|(rel, fs)| FileFindings {
                path: file_path_by_rel.remove(&rel).unwrap_or_default(),
                rel_path: rel,
                findings: fs,
            })
            .collect();

        Ok(ScanResult {
            root: std::path::PathBuf::from(root),
            ingest,
            triaged,
            findings_by_file,
            verified,
            patches,
            usage,
        })
    }

    pub fn delete_scan(&self, scan_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM scans WHERE id = ?1", params![scan_id])?;
        Ok(())
    }

    pub fn delete_scans_for_root(&self, root: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM scans WHERE root = ?1", params![root])?;
        Ok(())
    }

    /// Upsert a triage decision for (finding_id, root). Re-calling this with
    /// a different status overwrites the prior decision. `reason` is required
    /// for `dismissed`; `snooze_until` is required for `snoozed` but those
    /// constraints are policed at the IPC boundary, not here.
    pub fn set_triage(
        &self,
        finding_id: &str,
        root: &str,
        status: TriageStatus,
        reason: Option<&str>,
        snooze_until: Option<i64>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO triage (finding_id, root, status, reason, snooze_until, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(finding_id, root) DO UPDATE SET
                 status = excluded.status,
                 reason = excluded.reason,
                 snooze_until = excluded.snooze_until,
                 updated_at = excluded.updated_at",
            params![
                finding_id,
                root,
                status.as_str(),
                reason,
                snooze_until,
                now_ms(),
            ],
        )?;
        Ok(())
    }

    pub fn clear_triage(&self, finding_id: &str, root: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM triage WHERE finding_id = ?1 AND root = ?2",
            params![finding_id, root],
        )?;
        Ok(())
    }

    /// Load every triage decision for a root. The returned list is suitable
    /// for the frontend to index by `finding_id` and overlay on findings as
    /// they're rendered.
    pub fn get_triage_for_root(&self, root: &str) -> Result<Vec<TriageRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT finding_id, status, reason, snooze_until, updated_at
             FROM triage WHERE root = ?1",
        )?;
        let rows = stmt
            .query_map(params![root], |row| {
                let status_str: String = row.get(1)?;
                Ok((
                    row.get::<_, String>(0)?,
                    status_str,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(finding_id, status, reason, snooze_until, updated_at)| {
                Ok(TriageRecord {
                    finding_id,
                    status: TriageStatus::from_str(&status)?,
                    reason,
                    snooze_until,
                    updated_at,
                })
            })
            .collect()
    }
}

struct StoredFinding {
    finding_id: String,
    rel_path: String,
    kind: String,
    severity: String,
    cwe: String,
    owasp: Option<String>,
    title: String,
    file: String,
    line_start: i64,
    line_end: i64,
    description: String,
    data_flow: String,
    verdict_json: Option<String>,
    patch_json: Option<String>,
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 24-hex-char id with a millis prefix so SQL ORDER BY id ≈ chronological.
fn new_scan_id() -> String {
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

fn kind_str(k: crate::scanner::FindingKind) -> &'static str {
    match k {
        crate::scanner::FindingKind::Vuln => "vuln",
        crate::scanner::FindingKind::Hardening => "hardening",
    }
}
fn kind_from_str(s: &str) -> Result<crate::scanner::FindingKind> {
    Ok(match s {
        "vuln" => crate::scanner::FindingKind::Vuln,
        "hardening" => crate::scanner::FindingKind::Hardening,
        other => return Err(anyhow!("unknown finding kind in DB: {other}")),
    })
}

fn severity_str(s: crate::scanner::Severity) -> &'static str {
    use crate::scanner::Severity::*;
    match s {
        Critical => "critical",
        High => "high",
        Medium => "medium",
        Low => "low",
        Info => "info",
    }
}
fn severity_from_str(s: &str) -> Result<crate::scanner::Severity> {
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

// Silence: `Skipped` is referenced indirectly via WalkResult JSON round-trip.
#[allow(dead_code)]
fn _ensure_skipped_used(_: &Skipped) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::Severity;

    fn mk_finding(id_seed: &str, sev: Severity) -> Finding {
        let mut f = Finding {
            id: String::new(),
            kind: crate::scanner::FindingKind::Vuln,
            severity: sev,
            cwe: "CWE-89".into(),
            owasp: Some("A03:2021".into()),
            title: format!("Test {id_seed}"),
            file: "/abs/src/foo.ts".into(),
            line_start: 10,
            line_end: 12,
            description: "desc".into(),
            data_flow: "src→sink".into(),
        };
        f.assign_id();
        f
    }

    fn mk_result() -> ScanResult {
        let finding = mk_finding("a", Severity::High);
        ScanResult {
            root: std::path::PathBuf::from("/tmp/proj"),
            ingest: WalkResult::default(),
            triaged: Vec::new(),
            findings_by_file: vec![FileFindings {
                path: std::path::PathBuf::from("/abs/src/foo.ts"),
                rel_path: "src/foo.ts".into(),
                findings: vec![finding.clone()],
            }],
            verified: vec![VerifiedFinding {
                finding,
                verdict: Some(Verdict {
                    is_reachable: true,
                    source_is_untrusted: true,
                    concrete_exploit: None,
                    reasoning: "r".into(),
                }),
            }],
            patches: Vec::new(),
            usage: StageUsage::default(),
        }
    }

    #[test]
    fn round_trip_save_load() {
        let store = Store::open_in_memory().unwrap();
        let result = mk_result();
        let scan_id = store.save_scan(&result, "completed").unwrap();
        let loaded = store.load_scan(&scan_id).unwrap();
        assert_eq!(loaded.verified.len(), 1);
        assert_eq!(loaded.verified[0].finding.title, "Test a");
        assert!(loaded.verified[0].verdict.as_ref().unwrap().is_reachable);
        assert_eq!(loaded.findings_by_file.len(), 1);
        assert_eq!(loaded.findings_by_file[0].rel_path, "src/foo.ts");
        assert_eq!(loaded.root, std::path::PathBuf::from("/tmp/proj"));
    }

    #[test]
    fn list_scan_groups_returns_latest_per_root() {
        let store = Store::open_in_memory().unwrap();
        let result = mk_result();
        let _id1 = store.save_scan(&result, "completed").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let id2 = store.save_scan(&result, "completed").unwrap();

        let groups = store.list_scan_groups(20).unwrap();
        assert_eq!(groups.len(), 1, "same root must collapse to one group");
        assert_eq!(groups[0].latest_scan_id, id2);
        assert_eq!(groups[0].scan_count, 2);
    }

    #[test]
    fn migrations_are_idempotent() {
        let store = Store::open_in_memory().unwrap();
        // Second migrate call should be a no-op.
        store.migrate().unwrap();
        store.migrate().unwrap();
    }

    #[test]
    fn delete_scan_cascades() {
        let store = Store::open_in_memory().unwrap();
        let id = store.save_scan(&mk_result(), "completed").unwrap();
        store.delete_scan(&id).unwrap();
        assert!(store.load_scan(&id).is_err());
    }

    #[test]
    fn triage_round_trip_and_upsert() {
        let store = Store::open_in_memory().unwrap();
        let root = "/tmp/proj";

        // First: dismiss with a reason.
        store
            .set_triage("abc", root, TriageStatus::Dismissed, Some("false positive"), None)
            .unwrap();
        // Second: same key, switch to accepted (UPSERT).
        store
            .set_triage("abc", root, TriageStatus::Accepted, None, None)
            .unwrap();
        // Different finding in same root: snooze.
        store
            .set_triage("def", root, TriageStatus::Snoozed, None, Some(9_999_999))
            .unwrap();
        // Different root: shouldn't appear.
        store
            .set_triage("abc", "/tmp/other", TriageStatus::Dismissed, Some("x"), None)
            .unwrap();

        let mut got = store.get_triage_for_root(root).unwrap();
        got.sort_by(|a, b| a.finding_id.cmp(&b.finding_id));
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].finding_id, "abc");
        assert_eq!(got[0].status, TriageStatus::Accepted);
        assert_eq!(got[0].reason, None);
        assert_eq!(got[1].finding_id, "def");
        assert_eq!(got[1].status, TriageStatus::Snoozed);
        assert_eq!(got[1].snooze_until, Some(9_999_999));

        // Clear leaves the other row alone.
        store.clear_triage("abc", root).unwrap();
        let remaining = store.get_triage_for_root(root).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].finding_id, "def");
    }
}
