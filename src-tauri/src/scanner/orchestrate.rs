//! End-to-end pipeline orchestrator. Chains ingest → triage → detect →
//! verify → patch with the concurrency caps locked in CLAUDE.md, and
//! produces a `ScanResult` carrying every intermediate stage's output so
//! later the UI can render funnels and per-file detail without re-running
//! anything.
//!
//! Cancellation, budget caps, and >1000-file confirmation are intentionally
//! out of scope here — the CLI just warns; those belong with the UI in
//! Step 8.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{info, instrument, warn};

use crate::providers::Provider;
use crate::scanner::detect::{self, scan_with_tools};
use crate::scanner::ingest::{self, Candidate, WalkResult};
use crate::scanner::patch::{self, propose_many, Patch};
use crate::scanner::triage::{self, triage_many, Priority, TriagedFile};
use crate::scanner::verify::{self, verify_many, VerifiedFinding};
use crate::scanner::{Finding, FindingKind};

/// Tuning knobs for a scan. Defaults follow the locked decisions in
/// CLAUDE.md; callers can override per-stage if needed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanConfig {
    pub triage_concurrency: usize,
    pub detect_concurrency: usize,
    pub verify_concurrency: usize,
    pub patch_concurrency: usize,
    pub triage_model: String,
    pub detect_model: String,
    pub verify_model: String,
    pub patch_model: String,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            triage_concurrency: triage::DEFAULT_TRIAGE_CONCURRENCY,
            detect_concurrency: 4,
            verify_concurrency: verify::DEFAULT_VERIFY_CONCURRENCY,
            patch_concurrency: patch::DEFAULT_PATCH_CONCURRENCY,
            triage_model: triage::DEFAULT_TRIAGE_MODEL.into(),
            detect_model: detect::DEFAULT_DETECT_MODEL.into(),
            verify_model: verify::DEFAULT_VERIFY_MODEL.into(),
            patch_model: patch::DEFAULT_PATCH_MODEL.into(),
        }
    }
}

/// Detection output for one file, kept attached to the file path so the UI
/// can group findings by file in the left pane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileFindings {
    pub path: PathBuf,
    pub rel_path: String,
    pub findings: Vec<Finding>,
}

/// Everything the pipeline produced. Intermediate stages are retained so
/// the UI can render the triage funnel, verify decisions, etc., without
/// re-running anything.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    pub root: PathBuf,
    pub ingest: WalkResult,
    pub triaged: Vec<TriagedFile>,
    pub findings_by_file: Vec<FileFindings>,
    pub verified: Vec<VerifiedFinding>,
    pub patches: Vec<Patch>,
}

/// Run the full pipeline on a folder. Stages execute sequentially (so each
/// has the full output of the previous), but work *within* each stage is
/// parallelized under per-stage semaphores.
#[instrument(skip(provider, config), fields(root = %root.display()))]
pub async fn run_scan(
    root: PathBuf,
    provider: Arc<dyn Provider>,
    config: &ScanConfig,
) -> Result<ScanResult> {
    // ----- 1. Ingest --------------------------------------------------
    let ingest = ingest::walk(&root).context("walking scan root")?;
    info!(
        candidates = ingest.candidates.len(),
        skipped = ingest.skipped.len(),
        "ingest complete"
    );
    if ingest.candidates.len() > 1000 {
        warn!(
            count = ingest.candidates.len(),
            "scanning over 1000 files — this will take a while and cost real money"
        );
    }

    if ingest.candidates.is_empty() {
        return Ok(ScanResult {
            root,
            ingest,
            triaged: Vec::new(),
            findings_by_file: Vec::new(),
            verified: Vec::new(),
            patches: Vec::new(),
        });
    }

    // ----- 2. Triage --------------------------------------------------
    let triaged = triage_many(
        ingest.candidates.clone(),
        provider.clone(),
        &config.triage_model,
        config.triage_concurrency,
    )
    .await;
    let to_detect: Vec<Candidate> = triaged
        .iter()
        .filter(|t| t.result.priority != Priority::Skip)
        .map(|t| t.candidate.clone())
        .collect();
    info!(
        total = triaged.len(),
        keepers = to_detect.len(),
        "triage complete"
    );

    // ----- 3. Detect (parallel under Semaphore) -----------------------
    let detect_permits = Arc::new(Semaphore::new(config.detect_concurrency.max(1)));
    let mut set: JoinSet<DetectOutcome> = JoinSet::new();
    let detect_root = Arc::new(root.clone());
    let detect_model = Arc::new(config.detect_model.clone());
    for cand in to_detect {
        let permits = detect_permits.clone();
        let provider = provider.clone();
        let root = detect_root.clone();
        let model = detect_model.clone();
        set.spawn(async move {
            let _permit = match permits.acquire_owned().await {
                Ok(p) => p,
                Err(_) => return DetectOutcome::error(cand, "semaphore closed"),
            };
            let source = match tokio::fs::read_to_string(&cand.path).await {
                Ok(s) => s,
                Err(e) => return DetectOutcome::error(cand, &format!("read failed: {e}")),
            };
            match scan_with_tools(&cand.path, root.as_ref(), &source, provider.as_ref(), &model)
                .await
            {
                Ok(findings) => DetectOutcome::ok(cand, findings),
                Err(e) => DetectOutcome::error(cand, &format!("detect failed: {e:#}")),
            }
        });
    }
    let mut findings_by_file: Vec<FileFindings> = Vec::new();
    let mut all_findings: Vec<Finding> = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok(outcome) = joined {
            match outcome {
                DetectOutcome::Ok { cand, findings } => {
                    all_findings.extend(findings.iter().cloned());
                    findings_by_file.push(FileFindings {
                        path: cand.path,
                        rel_path: cand.rel_path,
                        findings,
                    });
                }
                DetectOutcome::Error { cand, error } => {
                    warn!(file = %cand.rel_path, error = %error, "detect errored; skipping file");
                }
            }
        }
    }
    findings_by_file.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    info!(
        files = findings_by_file.len(),
        findings = all_findings.len(),
        "detect complete"
    );

    // ----- 4. Verify --------------------------------------------------
    let verified = if all_findings.is_empty() {
        Vec::new()
    } else {
        verify_many(
            all_findings,
            root.clone(),
            provider.clone(),
            &config.verify_model,
            config.verify_concurrency,
        )
        .await
    };
    let kept_or_hardening = verified
        .iter()
        .filter(|v| {
            matches!(v.finding.kind, FindingKind::Hardening)
                || v.verdict.as_ref().map(|x| x.keep()).unwrap_or(false)
        })
        .count();
    info!(
        total = verified.len(),
        patchable = kept_or_hardening,
        "verify complete"
    );

    // ----- 5. Patch ---------------------------------------------------
    let patches = if kept_or_hardening == 0 {
        Vec::new()
    } else {
        propose_many(
            verified.clone(),
            root.clone(),
            provider,
            &config.patch_model,
            config.patch_concurrency,
        )
        .await
    };
    info!(count = patches.len(), "patch complete");

    Ok(ScanResult {
        root,
        ingest,
        triaged,
        findings_by_file,
        verified,
        patches,
    })
}

enum DetectOutcome {
    Ok {
        cand: Candidate,
        findings: Vec<Finding>,
    },
    Error {
        cand: Candidate,
        error: String,
    },
}

impl DetectOutcome {
    fn ok(cand: Candidate, findings: Vec<Finding>) -> Self {
        Self::Ok { cand, findings }
    }
    fn error(cand: Candidate, error: &str) -> Self {
        Self::Error {
            cand,
            error: error.to_string(),
        }
    }
}
