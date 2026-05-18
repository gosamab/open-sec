//! `scans` and `findings` table operations: persisting a finished pipeline
//! run, hydrating it back, and the launcher's list/lookup queries.

use anyhow::{anyhow, Result};
use rusqlite::{params, OptionalExtension};
use serde_json as json;

use crate::scanner::ingest::WalkResult;
use crate::scanner::orchestrate::{
    DetectError, FileFindings, ScanResult, ScanStatus, StageDurations, StageUsage,
};
use crate::scanner::patch::Patch;
use crate::scanner::triage::TriagedFile;
use crate::scanner::verify::{Verdict, VerifiedFinding};
use crate::scanner::Finding;

use super::types::{
    kind_from_str, kind_str, severity_from_str, severity_str, ScanGroup, StoredFinding,
};
use super::Store;

impl Store {
    /// Upsert a scan row plus its findings. Idempotent — `run_pipeline`
    /// calls this on every stage event with a freshly-built partial
    /// `ScanResult`, so an interrupted scan leaves the work-so-far on disk
    /// instead of vaporising every dollar spent up to that point. The final
    /// call from `run_pipeline` carries the authoritative result + a
    /// `completed` / `cancelled` status.
    ///
    /// Findings are sourced from `result.findings_by_file` (which is filled
    /// in by detect, before verify runs) so a scan that crashes mid-verify
    /// still carries every detected finding. Verdicts and patches overlay
    /// on top, so partial state shows up as findings without verdicts/patches
    /// rather than findings missing entirely.
    pub fn save_scan(
        &self,
        scan_id: &str,
        started_at: i64,
        result: &ScanResult,
        status: &str,
    ) -> Result<()> {
        let mut conn = self.db();
        let tx = conn.transaction()?;

        let walk_json = json::to_string(&result.ingest)?;
        let triaged_json = json::to_string(&result.triaged)?;
        let usage_json = json::to_string(&result.usage)?;
        let detect_errors_json = json::to_string(&result.detect_errors)?;
        let durations_json = json::to_string(&result.durations)?;
        let total: i64 = result
            .findings_by_file
            .iter()
            .map(|ff| ff.findings.len() as i64)
            .sum();
        let kept = result
            .verified
            .iter()
            .filter(|v| v.verdict.as_ref().map(|x| x.keep()).unwrap_or(false))
            .count() as i64;

        // UPSERT — incremental saves overwrite themselves, started_at stays
        // pinned to the first insert.
        tx.execute(
            "INSERT INTO scans (id, root, started_at, status,
                 total_findings, kept_findings,
                 walk_json, triaged_json, usage_json, detect_errors_json, durations_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
                 status = excluded.status,
                 total_findings = excluded.total_findings,
                 kept_findings = excluded.kept_findings,
                 walk_json = excluded.walk_json,
                 triaged_json = excluded.triaged_json,
                 usage_json = excluded.usage_json,
                 detect_errors_json = excluded.detect_errors_json,
                 durations_json = excluded.durations_json",
            params![
                scan_id,
                result.root.to_string_lossy(),
                started_at,
                status,
                total,
                kept,
                walk_json,
                triaged_json,
                usage_json,
                detect_errors_json,
                durations_json,
            ],
        )?;

        let mut verdict_by_id: std::collections::HashMap<&str, Option<&Verdict>> =
            std::collections::HashMap::new();
        for v in &result.verified {
            verdict_by_id.insert(v.finding.id.as_str(), v.verdict.as_ref());
        }
        let mut patch_by_id: std::collections::HashMap<&str, &Patch> =
            std::collections::HashMap::new();
        for p in &result.patches {
            patch_by_id.insert(p.finding_id.as_str(), p);
        }

        // Replace findings rows for this scan. Cheaper than reconciling
        // per-row updates, and the volumes are small (≤ low thousands).
        tx.execute("DELETE FROM findings WHERE scan_id = ?1", params![scan_id])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO findings (scan_id, finding_id, rel_path, kind, severity, cwe, owasp,
                     title, file, line_start, line_end, description, data_flow,
                     verdict_json, patch_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            )?;
            for ff in &result.findings_by_file {
                for f in &ff.findings {
                    let verdict_json = match verdict_by_id.get(f.id.as_str()) {
                        Some(Some(v)) => Some(json::to_string(v)?),
                        _ => None,
                    };
                    let patch_json = match patch_by_id.get(f.id.as_str()) {
                        Some(p) => Some(json::to_string(p)?),
                        None => None,
                    };
                    stmt.execute(params![
                        scan_id,
                        f.id,
                        ff.rel_path,
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
        }

        tx.commit()?;
        Ok(())
    }

    /// One row per root, showing the latest scan's metadata.
    pub fn list_scan_groups(&self, limit: usize) -> Result<Vec<ScanGroup>> {
        let conn = self.db();
        let mut stmt = conn.prepare(
            "SELECT s.root, s.id, s.started_at, s.kept_findings
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
                    latest_kept: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Hydrate a `ScanResult` back from the database. Used when the launcher
    /// opens a past scan without re-running.
    pub fn load_scan(&self, scan_id: &str) -> Result<ScanResult> {
        let conn = self.db();

        let (root, walk_json, triaged_json, usage_json, detect_errors_json, durations_json, status_str): (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
        ) = conn
            .query_row(
                "SELECT root, walk_json, triaged_json, usage_json, detect_errors_json, durations_json, status FROM scans WHERE id = ?1",
                params![scan_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("scan {scan_id} not found"))?;

        let ingest: WalkResult = json::from_str(&walk_json)?;
        let triaged: Vec<TriagedFile> = json::from_str(&triaged_json)?;
        let usage: StageUsage = json::from_str(&usage_json).unwrap_or_default();
        let detect_errors: Vec<DetectError> =
            json::from_str(&detect_errors_json).unwrap_or_default();
        let durations: StageDurations = json::from_str(&durations_json).unwrap_or_default();
        let status = match status_str.as_str() {
            "cancelled" => ScanStatus::Cancelled,
            "running" => ScanStatus::Running,
            _ => ScanStatus::Completed,
        };

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
            detect_errors,
            verified,
            patches,
            usage,
            durations,
            status,
        })
    }

    /// Return the scan_id of the most-recent scan for `root`, or `None`.
    /// Used by the export commands to find which scan to render.
    pub fn latest_scan_id_for_root(&self, root: &str) -> Result<Option<String>> {
        let conn = self.db();
        let id: Option<String> = conn
            .query_row(
                "SELECT id FROM scans WHERE root = ?1 ORDER BY started_at DESC LIMIT 1",
                params![root],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(id)
    }

    pub fn delete_scans_for_root(&self, root: &str) -> Result<()> {
        let conn = self.db();
        conn.execute("DELETE FROM scans WHERE root = ?1", params![root])?;
        Ok(())
    }

    /// Flip a scan row's status without rewriting its payload. Used to
    /// finalize an interrupted run as cancelled when `run_pipeline`'s error
    /// path can't synthesize a full `ScanResult`.
    pub fn update_scan_status(&self, scan_id: &str, status: &str) -> Result<()> {
        let conn = self.db();
        conn.execute(
            "UPDATE scans SET status = ?1 WHERE id = ?2",
            params![status, scan_id],
        )?;
        Ok(())
    }
}
