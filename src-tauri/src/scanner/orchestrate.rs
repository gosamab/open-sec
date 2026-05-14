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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinSet;
use tracing::{info, instrument, warn};

use crate::providers::counting::{diff, CancellingProvider, CountingProvider, UsageCounter};
use crate::providers::{Provider, Usage};
use crate::scanner::detect::{self, scan_with_tools};
use crate::scanner::ingest::{self, Candidate, WalkResult};
use crate::scanner::patch::{self, propose_many, Patch};
use crate::scanner::triage::{self, triage_many, Priority, TriagedFile};
use crate::scanner::verify::{self, verify_many, VerifiedFinding};
use crate::scanner::{Finding, FindingKind};

/// Stage-level events emitted as the pipeline progresses. The UI subscribes
/// to these to drive live updates without waiting for the full scan to
/// finish.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScanEvent {
    Started {
        root: PathBuf,
    },
    IngestComplete {
        walk: WalkResult,
    },
    TriageComplete {
        triaged: Vec<TriagedFile>,
    },
    /// One detected file's findings landed. Emitted as each file finishes
    /// so the UI can populate the left pane progressively.
    DetectFileComplete {
        rel_path: String,
        findings: Vec<Finding>,
    },
    /// Detect failed on a file (read error, model error, JSON parse failure,
    /// tool-iteration cap exceeded). The UI surfaces this as a red badge so
    /// "scan errored" doesn't look identical to "scan returned 0 findings".
    DetectFileErrored {
        rel_path: String,
        error: String,
    },
    DetectComplete {
        total: usize,
    },
    VerifyComplete {
        verified: Vec<VerifiedFinding>,
    },
    PatchComplete {
        patches: Vec<Patch>,
    },
    /// Running token totals after each stage. Emitted alongside the stage's
    /// completion event so the UI can show a live cost indicator.
    UsageUpdate {
        usage: StageUsage,
    },
}

/// Token usage broken down by stage, plus the rolling total. Each per-stage
/// `Usage` is the delta attributable to that stage's API calls — useful
/// for understanding where the cost lands.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StageUsage {
    pub triage: Usage,
    pub detect: Usage,
    pub verify: Usage,
    pub patch: Usage,
    pub total: Usage,
}

/// Sender used to ferry `ScanEvent`s out of the pipeline. The `Option`
/// makes event emission opt-in — CLI usage can pass `None` and skip the
/// extra plumbing.
pub type EventSender = mpsc::UnboundedSender<ScanEvent>;

fn emit(events: Option<&EventSender>, ev: ScanEvent) {
    if let Some(tx) = events {
        // Drop on send failure — the receiver simply went away.
        let _ = tx.send(ev);
    }
}

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
    /// Combined input+output token cap across the whole scan. `None` =
    /// unlimited. When reached, the orchestrator flips the cancel flag at
    /// the next stage boundary and the scan terminates with whatever it had
    /// collected.
    #[serde(default)]
    pub budget_total_tokens: Option<u32>,
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
            budget_total_tokens: None,
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
    pub usage: StageUsage,
}

/// Run the full pipeline on a folder. Stages execute sequentially (so each
/// has the full output of the previous), but work *within* each stage is
/// parallelized under per-stage semaphores.
///
/// `events`, when provided, receives a `ScanEvent` at each stage boundary
/// (and once per file during detect). Send failures are silently dropped —
/// the receiver going away does not abort the scan.
#[instrument(skip(provider, config, events), fields(root = %root.display()))]
pub async fn run_scan(
    root: PathBuf,
    provider: Arc<dyn Provider>,
    config: &ScanConfig,
    events: Option<EventSender>,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<ScanResult> {
    let events = events.as_ref();
    emit(events, ScanEvent::Started { root: root.clone() });

    // Wrap the inbound provider so every `generate()` accrues into a shared
    // counter. We snapshot the counter at each stage boundary to attribute
    // tokens per stage. If a cancellation flag was supplied, also wrap so
    // every API call short-circuits once it flips — stages still drain their
    // already-spawned tasks, but new API round-trips fail fast.
    let counter = UsageCounter::new();
    let mut provider: Arc<dyn Provider> = Arc::new(CountingProvider::new(provider, counter.clone()));
    if let Some(flag) = cancel.clone() {
        provider = Arc::new(CancellingProvider::new(provider, flag));
    }
    let mut stage_usage = StageUsage::default();
    let mut snapshot_before_stage = counter.snapshot();

    let is_cancelled = || cancel.as_ref().map(|f| f.load(Ordering::Relaxed)).unwrap_or(false);
    let trip_budget_if_over = |total: &Usage| {
        if let (Some(cap), Some(flag)) = (config.budget_total_tokens, cancel.as_ref()) {
            let used = total.input_tokens + total.output_tokens;
            if used >= cap {
                if !flag.load(Ordering::Relaxed) {
                    warn!(used, cap, "token budget exceeded — cancelling scan");
                }
                flag.store(true, Ordering::SeqCst);
            }
        }
    };

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
    emit(
        events,
        ScanEvent::IngestComplete {
            walk: ingest.clone(),
        },
    );

    if ingest.candidates.is_empty() || is_cancelled() {
        return Ok(ScanResult {
            root,
            ingest,
            triaged: Vec::new(),
            findings_by_file: Vec::new(),
            verified: Vec::new(),
            patches: Vec::new(),
            usage: stage_usage,
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
    emit(
        events,
        ScanEvent::TriageComplete {
            triaged: triaged.clone(),
        },
    );
    {
        let after = counter.snapshot();
        stage_usage.triage = diff(&after, &snapshot_before_stage);
        stage_usage.total = after.clone();
        snapshot_before_stage = after;
        emit(
            events,
            ScanEvent::UsageUpdate {
                usage: stage_usage.clone(),
            },
        );
        trip_budget_if_over(&stage_usage.total);
    }

    if is_cancelled() {
        info!("scan cancelled after triage");
        return Ok(ScanResult {
            root,
            ingest,
            triaged,
            findings_by_file: Vec::new(),
            verified: Vec::new(),
            patches: Vec::new(),
            usage: stage_usage,
        });
    }

    // ----- 3. Detect (parallel under Semaphore, streaming per-file) ---
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
                    emit(
                        events,
                        ScanEvent::DetectFileComplete {
                            rel_path: cand.rel_path.clone(),
                            findings: findings.clone(),
                        },
                    );
                    all_findings.extend(findings.iter().cloned());
                    findings_by_file.push(FileFindings {
                        path: cand.path,
                        rel_path: cand.rel_path,
                        findings,
                    });
                }
                DetectOutcome::Error { cand, error } => {
                    warn!(file = %cand.rel_path, error = %error, "detect errored; skipping file");
                    emit(
                        events,
                        ScanEvent::DetectFileErrored {
                            rel_path: cand.rel_path,
                            error,
                        },
                    );
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
    emit(
        events,
        ScanEvent::DetectComplete {
            total: all_findings.len(),
        },
    );
    {
        let after = counter.snapshot();
        stage_usage.detect = diff(&after, &snapshot_before_stage);
        stage_usage.total = after.clone();
        snapshot_before_stage = after;
        emit(
            events,
            ScanEvent::UsageUpdate {
                usage: stage_usage.clone(),
            },
        );
        trip_budget_if_over(&stage_usage.total);
    }

    // ----- 4. Verify --------------------------------------------------
    let verified = if all_findings.is_empty() || is_cancelled() {
        if is_cancelled() {
            info!("scan cancelled after detect; skipping verify/patch");
        }
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
    emit(
        events,
        ScanEvent::VerifyComplete {
            verified: verified.clone(),
        },
    );
    {
        let after = counter.snapshot();
        stage_usage.verify = diff(&after, &snapshot_before_stage);
        stage_usage.total = after.clone();
        snapshot_before_stage = after;
        emit(
            events,
            ScanEvent::UsageUpdate {
                usage: stage_usage.clone(),
            },
        );
        trip_budget_if_over(&stage_usage.total);
    }

    // ----- 5. Patch ---------------------------------------------------
    let patches = if kept_or_hardening == 0 || is_cancelled() {
        if is_cancelled() {
            info!("scan cancelled after verify; skipping patch");
        }
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
    emit(
        events,
        ScanEvent::PatchComplete {
            patches: patches.clone(),
        },
    );
    {
        let after = counter.snapshot();
        stage_usage.patch = diff(&after, &snapshot_before_stage);
        stage_usage.total = after;
        emit(
            events,
            ScanEvent::UsageUpdate {
                usage: stage_usage.clone(),
            },
        );
        trip_budget_if_over(&stage_usage.total);
    }

    Ok(ScanResult {
        root,
        ingest,
        triaged,
        findings_by_file,
        verified,
        patches,
        usage: stage_usage,
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
