use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;
use tracing::{info, instrument, warn};

use crate::config;
use crate::providers::anthropic::AnthropicProvider;
use crate::providers::Provider;
use crate::export;
use crate::scanner::detect::{scan_with_tools, DEFAULT_DETECT_MODEL};
use crate::scanner::excerpts::{extract_from_str, Excerpt};
use crate::scanner::orchestrate::{run_scan, ScanConfig, ScanEvent, ScanResult};
use crate::scanner::patch::{
    locate, propose_one_with_history, Located, Patch, PatchProposal, DEFAULT_PATCH_MODEL,
};
use crate::scanner::verify::VerifiedFinding;
use crate::scanner::Finding;
use crate::store::{AppliedPatchRecord, ScanGroup, Store, TriageRecord, TriageStatus};

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
pub fn greet(name: String) -> Result<String, String> {
    info!(name = %name, "greet command invoked");
    Ok(format!("Hello, {name}! open-sec is alive."))
}

#[tauri::command]
pub fn has_anthropic_key() -> bool {
    config::has_anthropic_key()
}

#[tauri::command]
pub fn set_anthropic_key(key: String) -> Result<(), String> {
    config::store_anthropic_key(&key).map_err(|e| e.to_string())
}

/// Scan a single file using the full agent loop (tools enabled).
///
/// `scan_root` is optional; when omitted, the file's parent directory is used.
/// Tools are sandboxed to that root — they cannot read outside it.
#[tauri::command]
#[instrument(skip_all, fields(path = %path, scan_root = ?scan_root))]
pub async fn scan_file(
    path: String,
    scan_root: Option<String>,
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

    let api_key = config::load_anthropic_key().map_err(|e| e.to_string())?;
    let provider = AnthropicProvider::new(api_key).map_err(|e| e.to_string())?;

    scan_with_tools(&file, &root, &source, &provider, DEFAULT_DETECT_MODEL)
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Run the full pipeline on a directory. Streams per-stage progress to the
/// frontend via the "scan:event" Tauri event; resolves with the final
/// `ScanResult`. On success, persists the result to SQLite for later recall.
/// If `cancel_scan` is invoked while running, returns the partial result
/// with status `cancelled`.
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

    let api_key = config::load_anthropic_key().map_err(|e| e.to_string())?;
    let provider: Arc<dyn Provider> =
        Arc::new(AnthropicProvider::new(api_key).map_err(|e| e.to_string())?);

    let (tx, mut rx) = mpsc::unbounded_channel::<ScanEvent>();

    let app_for_events = app.clone();
    let forward_task = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if let Err(e) = app_for_events.emit("scan:event", &event) {
                info!(error = %e, "failed to emit scan event; frontend disconnected?");
            }
        }
    });

    let cancel_flag = cancel.install();
    let config = config.unwrap_or_default();
    let mut result = run_scan(root_path, provider, &config, Some(tx), Some(cancel_flag.clone()))
        .await
        .map_err(|e| {
            cancel.clear();
            format!("{e:#}")
        })?;

    let _ = forward_task.await;

    let was_cancelled = cancel_flag.load(Ordering::SeqCst);
    cancel.clear();
    result.status = if was_cancelled {
        crate::scanner::orchestrate::ScanStatus::Cancelled
    } else {
        crate::scanner::orchestrate::ScanStatus::Completed
    };

    match store.save_scan(&result, result.status.as_str()) {
        Ok(scan_id) => info!(scan_id = %scan_id, status = result.status.as_str(), "scan persisted"),
        Err(e) => warn!(error = %format!("{e:#}"), "failed to persist scan"),
    }

    Ok(result)
}

/// Flag the currently-running scan for cancellation. The pipeline will
/// finish the current API call, skip subsequent stages, and return
/// whatever it had collected.
#[tauri::command]
pub fn cancel_scan(cancel: State<'_, CancelHandle>) -> bool {
    cancel.cancel()
}

/// Read a code excerpt for the finding's line range. Uses tree-sitter to
/// locate the enclosing function/class when possible, falls back to a ±N
/// line window for unsupported languages or top-level code.
#[tauri::command]
pub fn get_excerpt(
    file: String,
    line_start: u32,
    line_end: u32,
) -> Result<Excerpt, String> {
    extract_from_str(&file, line_start, line_end).map_err(|e| format!("{e:#}"))
}

#[derive(serde::Serialize)]
pub struct ApplyPatchResult {
    pub located: Located,
    pub bytes_written: u64,
}

/// Regenerate a patch for an already-verified finding, asking the model
/// for a structurally different proposal than the previous attempts.
/// `prior_attempts` carries the proposals already shown to the user.
#[tauri::command]
pub async fn regenerate_patch(
    root: String,
    verified: VerifiedFinding,
    prior_attempts: Vec<PatchProposal>,
) -> Result<Patch, String> {
    let scan_root = PathBuf::from(&root);
    if !scan_root.is_dir() {
        return Err(format!("not a directory: {root}"));
    }
    let api_key = config::load_anthropic_key().map_err(|e| e.to_string())?;
    let provider: Arc<dyn Provider> =
        Arc::new(AnthropicProvider::new(api_key).map_err(|e| e.to_string())?);
    propose_one_with_history(
        &verified,
        &scan_root,
        provider.as_ref(),
        DEFAULT_PATCH_MODEL,
        &prior_attempts,
    )
    .await
    .map_err(|e| format!("{e:#}"))
}

/// Apply a patch to disk. Re-locates `old_block` in the current file
/// content (exact match first, then fuzzy whitespace-tolerant), splices
/// `new_block` in its place, and writes the result back. Fails cleanly
/// if the file has drifted such that `old_block` no longer matches —
/// no partial writes. Records the apply in SQLite so the UI badge
/// survives reloads.
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

/// IPC: find and load the most recent scan for `root`. Used by the report
/// window so it can pull a fresh scan without paging through every group.
#[tauri::command]
pub fn get_latest_scan_for(store: State<'_, Store>, root: String) -> Result<ScanResult, String> {
    let id = latest_scan_id_for(store.inner(), &root)?;
    store.load_scan(&id).map_err(|e| format!("{e:#}"))
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

/// Remove a single scan by id (cascades to its findings).
#[tauri::command]
pub fn delete_scan(store: State<'_, Store>, scan_id: String) -> Result<(), String> {
    store.delete_scan(&scan_id).map_err(|e| format!("{e:#}"))
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
    let parsed = match status.as_str() {
        "accepted" => TriageStatus::Accepted,
        "dismissed" => TriageStatus::Dismissed,
        "snoozed" => TriageStatus::Snoozed,
        other => return Err(format!("unknown triage status: {other}")),
    };
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

/// Open a URL in the user's default browser. Used by the markdown renderer
/// so clicking an LLM-generated link doesn't navigate the Tauri webview.
///
/// Scheme is whitelisted (`http`, `https`, `mailto`) — anything else (file://,
/// javascript:, custom protocols) is rejected here rather than relying on
/// the OS to refuse. macOS-only for v0.1: shells out to `open(1)`.
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
