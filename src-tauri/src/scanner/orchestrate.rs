//! End-to-end pipeline orchestrator. Chains ingest → triage → detect →
//! verify → patch with the concurrency caps locked in CLAUDE.md. The
//! returned `ScanResult` carries every intermediate stage's output so the
//! UI can render funnels and per-file detail without re-running anything.
//! Cancellation is cooperative (via the optional `AtomicBool`) and the
//! token budget cap trips that same flag at stage boundaries.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinSet;
use tracing::{info, instrument, warn};

use crate::providers::counting::{diff, CancellingProvider, CountingProvider, RetryingProvider, UsageCounter};
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
    /// One verifier task finished (success, hardening pass-through, or error).
    /// Emitted from inside `verify_many` so the UI can show `verifying M/N`
    /// progress like detect does, instead of freezing at the start label
    /// until the whole stage finishes.
    VerifyProgress {
        done: usize,
        total: usize,
    },
    VerifyComplete {
        verified: Vec<VerifiedFinding>,
    },
    /// One patch task finished. Same rationale as `VerifyProgress`.
    PatchProgress {
        done: usize,
        total: usize,
    },
    PatchComplete {
        patches: Vec<Patch>,
    },
    /// Running token totals after each stage. Emitted alongside the stage's
    /// completion event so the UI can show a live cost indicator.
    UsageUpdate {
        usage: StageUsage,
    },
    /// Anthropic returned 429 and the provider is sleeping before retrying.
    /// Emitted once per retry attempt so the UI can show "retrying in Xs"
    /// instead of stalling silently.
    RateLimited {
        /// 1-indexed attempt number.
        attempt: u32,
        retry_after_secs: u64,
    },
    /// Per-stage wall-clock durations, updated after each stage boundary.
    /// `total_ms` is the elapsed time since `run_scan` was entered.
    DurationsUpdate {
        durations: StageDurations,
    },
}

/// Wall-clock time spent in each pipeline stage, plus the running total.
/// Per-stage values are the elapsed time between that stage's start and
/// completion. `total_ms` is the cumulative scan duration up to the most
/// recent emission; once the scan finishes it equals end-to-end runtime.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StageDurations {
    pub ingest_ms: u64,
    pub triage_ms: u64,
    pub detect_ms: u64,
    pub verify_ms: u64,
    pub patch_ms: u64,
    pub total_ms: u64,
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

/// Sender used to ferry `ScanEvent`s out of the pipeline.
pub type EventSender = mpsc::UnboundedSender<ScanEvent>;

fn emit(events: &EventSender, ev: ScanEvent) {
    // Drop on send failure — the receiver simply went away.
    let _ = events.send(ev);
}

#[derive(Clone, Copy)]
enum Stage {
    Triage,
    Detect,
    Verify,
    Patch,
}

/// Snapshot the usage counter, slot the per-stage delta into `usage`, record
/// the stage's wall-clock duration, and emit the resulting `UsageUpdate` +
/// `DurationsUpdate` events. Resets `stage_started` so the next stage starts
/// timing from now.
#[allow(clippy::too_many_arguments)]
fn finish_stage(
    stage: Stage,
    counter: &UsageCounter,
    snapshot_before: &mut Usage,
    usage: &mut StageUsage,
    durations: &mut StageDurations,
    stage_started: &mut Instant,
    scan_started: Instant,
    events: &EventSender,
) {
    let after = counter.snapshot();
    let delta = diff(&after, snapshot_before);
    let elapsed = stage_started.elapsed().as_millis() as u64;
    match stage {
        Stage::Triage => {
            usage.triage = delta;
            durations.triage_ms = elapsed;
        }
        Stage::Detect => {
            usage.detect = delta;
            durations.detect_ms = elapsed;
        }
        Stage::Verify => {
            usage.verify = delta;
            durations.verify_ms = elapsed;
        }
        Stage::Patch => {
            usage.patch = delta;
            durations.patch_ms = elapsed;
        }
    }
    usage.total = after.clone();
    durations.total_ms = scan_started.elapsed().as_millis() as u64;
    *snapshot_before = after;
    *stage_started = Instant::now();
    emit(
        events,
        ScanEvent::UsageUpdate {
            usage: usage.clone(),
        },
    );
    emit(
        events,
        ScanEvent::DurationsUpdate {
            durations: durations.clone(),
        },
    );
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

/// A file the detect stage failed on. Carried through so re-opened past
/// scans can show "scan finished with errors" instead of pretending the
/// scan was clean.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectError {
    pub rel_path: String,
    pub error: String,
}

/// Final state of a scan, surfaced to the UI so it can render the
/// difference without re-deriving it from the cancel flag (which can race
/// late-arriving stage events). `Running` is the in-flight state — the
/// pipeline writes partial rows with this status as it goes, so a crash or
/// abrupt close leaves a recognisable "interrupted" record instead of
/// silently dropping every dollar spent up to that point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanStatus {
    Running,
    Completed,
    Cancelled,
}

impl ScanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ScanStatus::Running => "running",
            ScanStatus::Completed => "completed",
            ScanStatus::Cancelled => "cancelled",
        }
    }
}

impl Default for ScanStatus {
    fn default() -> Self {
        ScanStatus::Completed
    }
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
    #[serde(default)]
    pub detect_errors: Vec<DetectError>,
    pub verified: Vec<VerifiedFinding>,
    pub patches: Vec<Patch>,
    pub usage: StageUsage,
    #[serde(default)]
    pub durations: StageDurations,
    /// Filled in by the command layer just before the result is returned to
    /// the frontend. The orchestrator itself always leaves this as the
    /// default ("completed") — it has no privileged view of whether the
    /// cancel flag was ever flipped during the run.
    #[serde(default)]
    pub status: ScanStatus,
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
    events: EventSender,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<ScanResult> {
    emit(&events, ScanEvent::Started { root: root.clone() });

    // Layer decorators on the inbound provider. Order matters:
    //   - Retry sits innermost so its sleeps don't get token-counted.
    //   - Counting wraps it so all responses (post-retry) are tallied.
    //   - Cancellation wraps everything so a cancel-flip short-circuits
    //     before any retry decision.
    // Each stage's `Usage` is computed by snapshotting the counter at stage
    // boundaries and diffing. Already-running HTTP requests aren't aborted —
    // cancel takes effect at the next round-trip.
    let counter = UsageCounter::new();
    let retry_notify: crate::providers::counting::RetryNotify = {
        let events_clone = events.clone();
        Arc::new(move |dur: std::time::Duration, attempt: u32| {
            let _ = events_clone.send(ScanEvent::RateLimited {
                attempt,
                retry_after_secs: dur.as_secs(),
            });
        })
    };
    let mut retrying = RetryingProvider::new(provider).with_notify(retry_notify);
    if let Some(flag) = cancel.clone() {
        retrying = retrying.with_cancel(flag);
    }
    let mut provider: Arc<dyn Provider> = Arc::new(retrying);
    provider = Arc::new(CountingProvider::new(provider, counter.clone()));
    if let Some(flag) = cancel.clone() {
        provider = Arc::new(CancellingProvider::new(provider, flag));
    }
    let mut stage_usage = StageUsage::default();
    let mut snapshot_before_stage = counter.snapshot();
    let mut durations = StageDurations::default();
    let scan_started = Instant::now();
    let mut stage_started = scan_started;

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
        &events,
        ScanEvent::IngestComplete {
            walk: ingest.clone(),
        },
    );
    durations.ingest_ms = stage_started.elapsed().as_millis() as u64;
    durations.total_ms = scan_started.elapsed().as_millis() as u64;
    emit(
        &events,
        ScanEvent::DurationsUpdate {
            durations: durations.clone(),
        },
    );
    stage_started = Instant::now();

    if ingest.candidates.is_empty() || is_cancelled() {
        return Ok(ScanResult {
            root,
            ingest,
            triaged: Vec::new(),
            findings_by_file: Vec::new(),
            detect_errors: Vec::new(),
            verified: Vec::new(),
            patches: Vec::new(),
            usage: stage_usage,
            durations,
            status: ScanStatus::default(),
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
        &events,
        ScanEvent::TriageComplete {
            triaged: triaged.clone(),
        },
    );
    finish_stage(
        Stage::Triage,
        &counter,
        &mut snapshot_before_stage,
        &mut stage_usage,
        &mut durations,
        &mut stage_started,
        scan_started,
        &events,
    );
    trip_budget_if_over(&stage_usage.total);

    if is_cancelled() {
        info!("scan cancelled after triage");
        return Ok(ScanResult {
            root,
            ingest,
            triaged,
            findings_by_file: Vec::new(),
            detect_errors: Vec::new(),
            verified: Vec::new(),
            patches: Vec::new(),
            usage: stage_usage,
            durations,
            status: ScanStatus::default(),
        });
    }

    // ----- 3. Detect (parallel under Semaphore, streaming per-file) ---
    let detect_permits = Arc::new(Semaphore::new(config.detect_concurrency.max(1)));
    let mut set: JoinSet<(Candidate, Result<Vec<Finding>, String>)> = JoinSet::new();
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
                Err(_) => return (cand, Err("semaphore closed".to_string())),
            };
            let source = match tokio::fs::read_to_string(&cand.path).await {
                Ok(s) => s,
                Err(e) => return (cand, Err(format!("read failed: {e}"))),
            };
            let result =
                scan_with_tools(&cand.path, root.as_ref(), &source, provider.as_ref(), &model)
                    .await
                    .map_err(|e| format!("detect failed: {e:#}"));
            (cand, result)
        });
    }
    let mut findings_by_file: Vec<FileFindings> = Vec::new();
    let mut all_findings: Vec<Finding> = Vec::new();
    let mut detect_errors: Vec<DetectError> = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok((cand, outcome)) = joined {
            match outcome {
                Ok(findings) => {
                    emit(
                        &events,
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
                Err(error) => {
                    warn!(file = %cand.rel_path, error = %error, "detect errored; skipping file");
                    detect_errors.push(DetectError {
                        rel_path: cand.rel_path.clone(),
                        error: error.clone(),
                    });
                    emit(
                        &events,
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
    detect_errors.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    info!(
        files = findings_by_file.len(),
        findings = all_findings.len(),
        "detect complete"
    );
    emit(
        &events,
        ScanEvent::DetectComplete {
            total: all_findings.len(),
        },
    );
    finish_stage(
        Stage::Detect,
        &counter,
        &mut snapshot_before_stage,
        &mut stage_usage,
        &mut durations,
        &mut stage_started,
        scan_started,
        &events,
    );
    trip_budget_if_over(&stage_usage.total);

    // ----- 4. Verify --------------------------------------------------
    let verified = if all_findings.is_empty() || is_cancelled() {
        if is_cancelled() {
            info!("scan cancelled after detect; skipping verify/patch");
        }
        Vec::new()
    } else {
        let total = all_findings.len();
        emit(&events, ScanEvent::VerifyProgress { done: 0, total });
        let progress = {
            let events = events.clone();
            let done = Arc::new(AtomicUsize::new(0));
            let tick: verify::ProgressTick = Arc::new(move || {
                let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                let _ = events.send(ScanEvent::VerifyProgress { done: d, total });
            });
            Some(tick)
        };
        verify_many(
            all_findings,
            root.clone(),
            provider.clone(),
            &config.verify_model,
            config.verify_concurrency,
            progress,
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
        &events,
        ScanEvent::VerifyComplete {
            verified: verified.clone(),
        },
    );
    finish_stage(
        Stage::Verify,
        &counter,
        &mut snapshot_before_stage,
        &mut stage_usage,
        &mut durations,
        &mut stage_started,
        scan_started,
        &events,
    );
    trip_budget_if_over(&stage_usage.total);

    // ----- 5. Patch ---------------------------------------------------
    let patches = if kept_or_hardening == 0 || is_cancelled() {
        if is_cancelled() {
            info!("scan cancelled after verify; skipping patch");
        }
        Vec::new()
    } else {
        let total = kept_or_hardening;
        emit(&events, ScanEvent::PatchProgress { done: 0, total });
        let progress = {
            let events = events.clone();
            let done = Arc::new(AtomicUsize::new(0));
            let tick: verify::ProgressTick = Arc::new(move || {
                let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                let _ = events.send(ScanEvent::PatchProgress { done: d, total });
            });
            Some(tick)
        };
        propose_many(
            verified.clone(),
            root.clone(),
            provider,
            &config.patch_model,
            config.patch_concurrency,
            progress,
        )
        .await
    };
    info!(count = patches.len(), "patch complete");
    emit(
        &events,
        ScanEvent::PatchComplete {
            patches: patches.clone(),
        },
    );
    finish_stage(
        Stage::Patch,
        &counter,
        &mut snapshot_before_stage,
        &mut stage_usage,
        &mut durations,
        &mut stage_started,
        scan_started,
        &events,
    );
    trip_budget_if_over(&stage_usage.total);

    Ok(ScanResult {
        root,
        ingest,
        triaged,
        findings_by_file,
        detect_errors,
        verified,
        patches,
        usage: stage_usage,
        durations,
        status: ScanStatus::default(),
    })
}

