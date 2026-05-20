use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc;
use tracing::{info, instrument, warn};

use crate::config;
use crate::providers::anthropic::AnthropicProvider;
use crate::providers::multiplex::MultiplexProvider;
use crate::providers::openai::OpenAiProvider;
use crate::providers::rate_limit::MultiObserver;
use crate::providers::{route_model_to_provider, Provider};
use crate::export;
use crate::scanner::detect::{scan_with_tools, DEFAULT_DETECT_MODEL};
use crate::scanner::excerpts::{extract, Excerpt};
use crate::scanner::ingest::{self, WalkResult};
use crate::scanner::orchestrate::{
    run_scan, DetectError, FileFindings, ScanConfig, ScanEvent, ScanResult, ScanStatus,
    StageDurations, StageUsage,
};
use crate::scanner::triage::TriageError;
use crate::scanner::patch::{
    locate, propose_one, Located, Patch, PatchProposal, DEFAULT_PATCH_MODEL,
};
use crate::scanner::verify::VerifiedFinding;
use crate::scanner::Finding;
use crate::store::{
    new_scan_id, now_ms, AppliedPatchRecord, ScanGroup, Store, TriageRecord, TriageStatus,
};

/// Shared, app-wide cancel handle. Only one scan can run at a time (single
/// workspace UX), so a single slot is enough — when a new scan starts it
/// installs a fresh flag and replaces whatever was there.
#[derive(Default)]
pub struct CancelHandle {
    current: Mutex<Option<Arc<AtomicBool>>>,
}

impl CancelHandle {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Arc<AtomicBool>>> {
        self.current.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn install(&self) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        *self.lock() = Some(flag.clone());
        flag
    }

    pub fn clear(&self) {
        *self.lock() = None;
    }

    pub fn cancel(&self) -> bool {
        if let Some(flag) = self.lock().as_ref() {
            flag.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }
}

#[tauri::command]
pub fn has_anthropic_key() -> bool {
    config::has_anthropic_key()
}

#[tauri::command]
pub fn set_anthropic_key(key: String) -> Result<(), String> {
    config::store_anthropic_key(&key).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub fn has_openai_key() -> bool {
    config::has_openai_key()
}

#[tauri::command]
pub fn set_openai_key(key: String) -> Result<(), String> {
    config::store_openai_key(&key).map_err(|e| format!("{e:#}"))
}

/// Build a multiplex provider configured for the providers each stage needs.
/// Inspects `stage_models` (a list of `(stage_label, model)` pairs) to figure
/// out whether Anthropic, OpenAI, or both are required. Each required key is
/// loaded eagerly so a missing key is reported as a single, actionable error
/// before any LLM work begins (fail-fast gate). The returned provider is
/// rate-limit observed under the multi-provider observer.
fn build_multiplex_provider(
    observer: &Arc<MultiObserver>,
    stage_models: &[(&str, &str)],
) -> Result<Arc<dyn Provider>, String> {
    let mut needs_anthropic: Option<(&str, &str)> = None;
    let mut needs_openai: Option<(&str, &str)> = None;
    for (stage, model) in stage_models {
        match route_model_to_provider(model) {
            "anthropic" => {
                if needs_anthropic.is_none() {
                    needs_anthropic = Some((stage, model));
                }
            }
            "openai" => {
                if needs_openai.is_none() {
                    needs_openai = Some((stage, model));
                }
            }
            _ => {}
        }
    }

    let mut mux = MultiplexProvider::new();

    if let Some((stage, model)) = needs_anthropic {
        let key = config::load_anthropic_key().map_err(|_| {
            format!("{stage} uses model '{model}' but ANTHROPIC_API_KEY is not configured")
        })?;
        let provider = AnthropicProvider::new(key)
            .map_err(|e| e.to_string())?
            .with_rate_limit_observer(observer.clone());
        mux = mux.with_anthropic(Arc::new(provider));
    }

    if let Some((stage, model)) = needs_openai {
        let key = config::load_openai_key().map_err(|_| {
            format!("{stage} uses model '{model}' but OPENAI_API_KEY is not configured")
        })?;
        let provider = OpenAiProvider::new(key)
            .map_err(|e| e.to_string())?
            .with_rate_limit_observer(observer.clone());
        mux = mux.with_openai(Arc::new(provider));
    }

    Ok(Arc::new(mux))
}

/// Scan a single file using the full agent loop (tools enabled).
///
/// `scan_root` is optional; when omitted, the file's parent directory is used.
/// Tools are sandboxed to that root — they cannot read outside it. `model`
/// should match the user's configured `detect_model`; the multiplex routes
/// to the matching provider.
#[tauri::command]
#[instrument(skip_all, fields(path = %path, scan_root = ?scan_root))]
pub async fn scan_file(
    path: String,
    scan_root: Option<String>,
    model: Option<String>,
) -> Result<Vec<Finding>, String> {
    let file = PathBuf::from(&path);
    if !file.is_file() {
        return Err(format!("not a file: {path}"));
    }

    let root: PathBuf = match scan_root {
        Some(s) => PathBuf::from(s),
        None => file
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| format!("could not derive scan root from {}", file.display()))?,
    };

    let source = tokio::fs::read_to_string(&file)
        .await
        .map_err(|e| format!("read {}: {e}", file.display()))?;

    let model = model.unwrap_or_else(|| DEFAULT_DETECT_MODEL.to_string());
    let observer = MultiObserver::new();
    let provider = build_multiplex_provider(&observer, &[("detect_model", &model)])?;

    scan_with_tools(&file, &root, &source, provider.as_ref(), &model)
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Apply one `ScanEvent` to a partial `ScanResult` so the forwarder task
/// can persist as-we-go. Events that don't change persistent state
/// (`Started`, progress ticks, rate-limit notices) fall through to the
/// catch-all and are silently ignored — they're UI-only signals.
fn apply_event(partial: &mut ScanResult, event: &ScanEvent) {
    match event {
        ScanEvent::IngestComplete { walk } => partial.ingest = walk.clone(),
        ScanEvent::TriageComplete { triaged } => partial.triaged = triaged.clone(),
        ScanEvent::TriageFileErrored { rel_path, error } => {
            partial.triage_errors.push(TriageError {
                rel_path: rel_path.clone(),
                error: error.clone(),
            });
        }
        ScanEvent::DetectFileComplete { rel_path, findings } => {
            // The forwarder only sees rel_path; we don't have the absolute
            // path on this side. `save_scan` reads `rel_path` exclusively,
            // and the frontend treats `path` as a no-op decoration, so we
            // stuff rel_path in both. The final save from `run_pipeline`
            // overwrites this with the authoritative absolute path.
            partial.findings_by_file.push(FileFindings {
                path: PathBuf::from(rel_path),
                rel_path: rel_path.clone(),
                findings: findings.clone(),
            });
        }
        ScanEvent::DetectFileErrored { rel_path, error } => {
            partial.detect_errors.push(DetectError {
                rel_path: rel_path.clone(),
                error: error.clone(),
            });
        }
        ScanEvent::VerifyComplete { verified } => partial.verified = verified.clone(),
        ScanEvent::PatchComplete { patches } => partial.patches = patches.clone(),
        ScanEvent::UsageUpdate { usage } => partial.usage = usage.clone(),
        ScanEvent::DurationsUpdate { durations } => partial.durations = durations.clone(),
        _ => {}
    }
}

/// Shared body for `run_pipeline` (fresh scan) and `resume_pipeline`
/// (continuation of a previous interrupted scan). The only differences:
/// `scan_id` is new vs reused, and `previous` is `None` vs `Some(loaded)`.
/// Everything else — provider construction, incremental persistence,
/// final status assignment — is identical.
async fn drive_pipeline(
    app: AppHandle,
    store: &Store,
    cancel: &CancelHandle,
    scan_id: String,
    started_at: i64,
    root_path: PathBuf,
    config: ScanConfig,
    previous: Option<ScanResult>,
) -> Result<ScanResult, String> {
    let observer = MultiObserver::new();
    let provider = build_multiplex_provider(
        &observer,
        &[
            ("triage_model", &config.triage_model),
            ("detect_model", &config.detect_model),
            ("verify_model", &config.verify_model),
            ("patch_model", &config.patch_model),
        ],
    )?;

    // The initial DB row mirrors whatever state we're starting from: an
    // empty skeleton for a fresh scan, or the loaded partial for a resume.
    // For a resume the UPSERT's `ON CONFLICT` clause is a no-op for fields
    // we'd otherwise overwrite with identical values, but it pins
    // `status='running'` again immediately.
    let initial = previous.clone().unwrap_or_else(|| ScanResult {
        root: root_path.clone(),
        ingest: WalkResult::default(),
        triaged: Vec::new(),
        triage_errors: Vec::new(),
        findings_by_file: Vec::new(),
        detect_errors: Vec::new(),
        verified: Vec::new(),
        patches: Vec::new(),
        usage: StageUsage::default(),
        durations: StageDurations::default(),
        status: ScanStatus::Running,
    });
    if let Err(e) = store.save_scan(&scan_id, started_at, &initial, ScanStatus::Running.as_str()) {
        warn!(error = %format!("{e:#}"), "failed to insert initial scan row");
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<ScanEvent>();

    let app_for_events = app.clone();
    let scan_id_for_task = scan_id.clone();
    let initial_for_task = initial.clone();
    let forward_task = tokio::spawn(async move {
        let store: State<Store> = app_for_events.state();
        let mut partial = initial_for_task;
        while let Some(event) = rx.recv().await {
            apply_event(&mut partial, &event);
            if let Err(e) = store.save_scan(
                &scan_id_for_task,
                started_at,
                &partial,
                ScanStatus::Running.as_str(),
            ) {
                warn!(error = %format!("{e:#}"), "incremental save failed");
            }
            if let Err(e) = app_for_events.emit("scan:event", &event) {
                info!(error = %e, "failed to emit scan event; frontend disconnected?");
            }
        }
    });

    let cancel_flag = cancel.install();
    let mut result = run_scan(
        root_path,
        provider,
        &config,
        tx,
        Some(cancel_flag.clone()),
        previous,
        Some(observer),
    )
    .await
    .map_err(|e| {
        cancel.clear();
        // Best-effort: flip the in-progress row to cancelled so it doesn't
        // sit at "running" forever. The error path doesn't have a
        // ScanResult; the existing partial state is whatever forward_task
        // persisted last.
        let _ = store.update_scan_status(&scan_id, ScanStatus::Cancelled.as_str());
        format!("{e:#}")
    })?;

    let _ = forward_task.await;

    let was_cancelled = cancel_flag.load(Ordering::SeqCst);
    cancel.clear();
    result.status = if was_cancelled {
        ScanStatus::Cancelled
    } else {
        ScanStatus::Completed
    };

    // Authoritative final save with absolute paths, full state, terminal status.
    match store.save_scan(&scan_id, started_at, &result, result.status.as_str()) {
        Ok(_) => info!(scan_id = %scan_id, status = result.status.as_str(), "scan persisted"),
        Err(e) => warn!(error = %format!("{e:#}"), "failed to finalize scan"),
    }

    Ok(result)
}

/// Run the full pipeline on a directory. Per-stage progress streams via the
/// `scan:event` Tauri event and is incrementally persisted to SQLite as
/// each event arrives, so a crash or abrupt close leaves the work-so-far
/// on disk. `cancel_scan` makes this return the partial result with
/// `status = cancelled`.
#[tauri::command]
#[instrument(skip(app, store, cancel), fields(root = %root))]
pub async fn run_pipeline(
    app: AppHandle,
    store: State<'_, Store>,
    cancel: State<'_, CancelHandle>,
    root: String,
    config: Option<ScanConfig>,
) -> Result<ScanResult, String> {
    let root_path = PathBuf::from(&root);
    if !root_path.is_dir() {
        return Err(format!("not a directory: {root}"));
    }
    drive_pipeline(
        app,
        store.inner(),
        cancel.inner(),
        new_scan_id(),
        now_ms(),
        root_path,
        config.unwrap_or_default(),
        None,
    )
    .await
}

/// Resume a previously-interrupted scan. Loads the partial `ScanResult`
/// from SQLite and re-runs the orchestrator with `previous` set so each
/// stage skips work that's already done — only files without cached
/// findings get re-detected, only findings without verdicts get re-verified,
/// only patchable findings without patches get re-proposed. Cached state
/// is re-emitted on the event stream so the UI hydrates the full picture.
#[tauri::command]
#[instrument(skip(app, store, cancel), fields(scan_id = %scan_id))]
pub async fn resume_pipeline(
    app: AppHandle,
    store: State<'_, Store>,
    cancel: State<'_, CancelHandle>,
    scan_id: String,
    config: Option<ScanConfig>,
) -> Result<ScanResult, String> {
    let previous = store
        .load_scan(&scan_id)
        .map_err(|e| format!("load scan {scan_id}: {e:#}"))?;
    let root_path = previous.root.clone();
    if !root_path.is_dir() {
        return Err(format!(
            "scan root no longer exists: {}",
            root_path.display()
        ));
    }
    // started_at is preserved by save_scan's UPSERT — the row already
    // exists, so the value we pass here goes unused.
    drive_pipeline(
        app,
        store.inner(),
        cancel.inner(),
        scan_id,
        now_ms(),
        root_path,
        config.unwrap_or_default(),
        Some(previous),
    )
    .await
}

/// Flag the running scan for cancellation. The pipeline finishes its
/// current API call, skips later stages, and returns the partial result.
#[tauri::command]
pub fn cancel_scan(cancel: State<'_, CancelHandle>) -> bool {
    cancel.cancel()
}

/// Walk `root` and return the candidate/skipped split without running any
/// LLM stage. Drives the pre-scan cost estimate on the onboarding panel —
/// the frontend sums `line_count` across candidates and prices it against
/// the configured per-stage models.
#[tauri::command]
#[instrument(skip_all, fields(root = %root))]
pub fn estimate_scan(root: String) -> Result<WalkResult, String> {
    let root_path = PathBuf::from(&root);
    if !root_path.is_dir() {
        return Err(format!("not a directory: {root}"));
    }
    ingest::walk(&root_path).map_err(|e| format!("{e:#}"))
}

/// Code excerpt for a finding's line range. Tree-sitter languages get the
/// enclosing function/class; others fall back to a ±N line window.
#[tauri::command]
pub fn get_excerpt(
    file: String,
    line_start: u32,
    line_end: u32,
) -> Result<Excerpt, String> {
    extract(&PathBuf::from(&file), line_start, line_end).map_err(|e| format!("{e:#}"))
}

#[derive(serde::Serialize)]
pub struct ApplyPatchResult {
    pub located: Located,
    pub bytes_written: u64,
}

/// Regenerate a patch for an already-verified finding, asking the model
/// for a structurally different proposal than the previous attempts.
/// `prior_attempts` carries the proposals already shown to the user.
/// `model` is the patch_model from the user's settings — routed to the
/// matching provider via the multiplex.
#[tauri::command]
pub async fn regenerate_patch(
    root: String,
    verified: VerifiedFinding,
    prior_attempts: Vec<PatchProposal>,
    model: Option<String>,
) -> Result<Patch, String> {
    let scan_root = PathBuf::from(&root);
    if !scan_root.is_dir() {
        return Err(format!("not a directory: {root}"));
    }
    let model = model.unwrap_or_else(|| DEFAULT_PATCH_MODEL.to_string());
    let observer = MultiObserver::new();
    let provider = build_multiplex_provider(&observer, &[("patch_model", &model)])?;
    propose_one(
        &verified,
        &scan_root,
        provider.as_ref(),
        &model,
        &prior_attempts,
    )
    .await
    .map_err(|e| format!("{e:#}"))
}

/// Apply a patch to disk. Re-locates `old_block` (exact, then fuzzy),
/// splices `new_block`, and writes the result back. Fails clean — no
/// partial writes — if the file has drifted since the patch was drafted.
/// Records the apply in SQLite so the UI badge survives reload.
#[tauri::command]
pub fn apply_patch(
    store: State<'_, Store>,
    finding_id: String,
    root: String,
    file: String,
    old_block: String,
    new_block: String,
) -> Result<ApplyPatchResult, String> {
    let path = PathBuf::from(&file);
    if !path.is_file() {
        return Err(format!("not a file: {file}"));
    }
    let source = std::fs::read_to_string(&path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;

    let located = locate(&source, &old_block);
    let (off, len) = match &located {
        Located::Exact { byte_offset, .. } => (*byte_offset, old_block.len()),
        Located::Fuzzy {
            byte_offset,
            matched_text,
            ..
        } => (*byte_offset, matched_text.len()),
        Located::NotFound => {
            return Err(
                "old_block not located in current file — the file may have changed since the patch was generated"
                    .into(),
            );
        }
    };

    let mut modified = String::with_capacity(source.len() + new_block.len());
    modified.push_str(&source[..off]);
    modified.push_str(&new_block);
    modified.push_str(&source[off + len..]);

    let bytes_written = modified.len() as u64;
    std::fs::write(&path, &modified)
        .map_err(|e| format!("write {}: {e}", path.display()))?;

    if let Err(e) = store.record_patch_applied(&finding_id, &root, &file) {
        warn!(error = %format!("{e:#}"), "failed to record applied patch in store");
    }
    info!(file = %path.display(), bytes_written, %finding_id, "patch applied");
    Ok(ApplyPatchResult {
        located,
        bytes_written,
    })
}

/// List every finding whose patch has been applied (in this root). The UI
/// uses this on hydration to restore the applied badge after a reload.
#[tauri::command]
pub fn get_applied_for_root(
    store: State<'_, Store>,
    root: String,
) -> Result<Vec<AppliedPatchRecord>, String> {
    store
        .get_applied_for_root(&root)
        .map_err(|e| format!("{e:#}"))
}

/// Find the most recent scan for `root` and return its scan_id. Backed by a
/// targeted SQL query so it doesn't depend on `list_scan_groups`'s page size.
fn latest_scan_id_for(store: &Store, root: &str) -> Result<String, String> {
    store
        .latest_scan_id_for_root(root)
        .map_err(|e| format!("{e:#}"))?
        .ok_or_else(|| format!("no persisted scan found for {root}"))
}

/// Render a markdown report for the latest persisted scan of `root`.
#[tauri::command]
pub fn export_markdown(store: State<'_, Store>, root: String) -> Result<String, String> {
    let id = latest_scan_id_for(store.inner(), &root)?;
    let result = store.load_scan(&id).map_err(|e| format!("{e:#}"))?;
    Ok(export::export_markdown(&result))
}

/// Render a SARIF v2.1.0 document for the latest persisted scan of `root`.
#[tauri::command]
pub fn export_sarif(store: State<'_, Store>, root: String) -> Result<String, String> {
    let id = latest_scan_id_for(store.inner(), &root)?;
    let result = store.load_scan(&id).map_err(|e| format!("{e:#}"))?;
    Ok(export::export_sarif(&result))
}

/// Write a UTF-8 file to disk. Used by the export buttons to save the
/// content returned by `export_markdown` / `export_sarif` to a user-picked
/// path — avoids needing a permissive `fs` scope on the frontend.
#[tauri::command]
pub fn save_text_file(path: String, content: String) -> Result<(), String> {
    std::fs::write(&path, &content).map_err(|e| format!("write {path}: {e}"))
}

/// List the most recent scan groups (one row per root) for the launcher.
#[tauri::command]
pub fn list_scan_groups(
    store: State<'_, Store>,
    limit: Option<usize>,
) -> Result<Vec<ScanGroup>, String> {
    store
        .list_scan_groups(limit.unwrap_or(20))
        .map_err(|e| format!("{e:#}"))
}

/// Reload a past scan into a `ScanResult` for the workspace.
#[tauri::command]
pub fn load_scan(store: State<'_, Store>, scan_id: String) -> Result<ScanResult, String> {
    store.load_scan(&scan_id).map_err(|e| format!("{e:#}"))
}

/// Remove every scan for a given root — used by the launcher's "remove from
/// recents" action.
#[tauri::command]
pub fn delete_scans_for_root(store: State<'_, Store>, root: String) -> Result<(), String> {
    store
        .delete_scans_for_root(&root)
        .map_err(|e| format!("{e:#}"))
}

/// Set (or upsert) a triage decision on a finding. `status` is one of
/// `accepted`, `dismissed`, `snoozed`. `reason` is required for `dismissed`;
/// `snooze_until` is required for `snoozed`.
#[tauri::command]
pub fn set_triage(
    store: State<'_, Store>,
    finding_id: String,
    root: String,
    status: String,
    reason: Option<String>,
    snooze_until: Option<i64>,
) -> Result<(), String> {
    let parsed = TriageStatus::from_str(&status).map_err(|e| e.to_string())?;
    if parsed == TriageStatus::Dismissed && reason.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err("dismissed requires a non-empty reason".into());
    }
    if parsed == TriageStatus::Snoozed && snooze_until.is_none() {
        return Err("snoozed requires snooze_until (unix ms)".into());
    }
    store
        .set_triage(&finding_id, &root, parsed, reason.as_deref(), snooze_until)
        .map_err(|e| format!("{e:#}"))
}

/// Remove a triage decision so the finding is unmarked again.
#[tauri::command]
pub fn clear_triage(
    store: State<'_, Store>,
    finding_id: String,
    root: String,
) -> Result<(), String> {
    store
        .clear_triage(&finding_id, &root)
        .map_err(|e| format!("{e:#}"))
}

/// Load every triage decision for a root.
#[tauri::command]
pub fn get_triage_for_root(
    store: State<'_, Store>,
    root: String,
) -> Result<Vec<TriageRecord>, String> {
    store
        .get_triage_for_root(&root)
        .map_err(|e| format!("{e:#}"))
}

/// Open a file in VS Code at the given line via the `vscode://file/...:line`
/// URL handler — same handler Cursor / VSCodium register, so it works for
/// most VS Code-derived editors without per-editor config. macOS-only:
/// shells out to `open(1)`, which routes the URL to whatever app claims it.
/// Path is canonicalized and confirmed to exist first; we never pass raw
/// user input into the URL.
#[tauri::command]
pub fn open_in_editor(path: String, line: Option<u32>) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.is_file() {
        return Err(format!("not a file: {path}"));
    }
    let canonical = p.canonicalize().map_err(|e| format!("canonicalize {path}: {e}"))?;
    let url = match line {
        Some(l) if l > 0 => format!("vscode://file{}:{}", canonical.display(), l),
        _ => format!("vscode://file{}", canonical.display()),
    };
    std::process::Command::new("open")
        .arg(&url)
        .spawn()
        .map_err(|e| format!("open failed: {e}"))?;
    Ok(())
}

/// Open a URL in the user's default browser. Scheme is whitelisted
/// (http/https/mailto) so an LLM-generated link can't reach file://,
/// javascript:, or custom protocols. macOS-only: shells out to `open(1)`.
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("empty url".into());
    }
    let lower = trimmed.to_ascii_lowercase();
    let allowed = lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:");
    if !allowed {
        return Err(format!("scheme not allowed: {trimmed}"));
    }
    if trimmed.len() > 4096 {
        return Err("url too long".into());
    }
    std::process::Command::new("open")
        .arg(trimmed)
        .spawn()
        .map_err(|e| format!("open failed: {e}"))?;
    Ok(())
}
