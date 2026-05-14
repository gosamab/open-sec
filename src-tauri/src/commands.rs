use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;
use tracing::{info, instrument, warn};

use crate::config;
use crate::providers::anthropic::AnthropicProvider;
use crate::providers::Provider;
use crate::scanner::detect::{scan_with_tools, DEFAULT_DETECT_MODEL};
use crate::scanner::orchestrate::{run_scan, ScanConfig, ScanEvent, ScanResult};
use crate::scanner::Finding;
use crate::store::{ScanGroup, Store, TriageRecord, TriageStatus};

/// Shared, app-wide cancel handle. Only one scan can run at a time (single
/// workspace UX), so a single slot is enough — when a new scan starts it
/// installs a fresh flag and replaces whatever was there.
#[derive(Default)]
pub struct CancelHandle {
    current: Mutex<Option<Arc<AtomicBool>>>,
}

impl CancelHandle {
    pub fn install(&self) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        *self.current.lock().unwrap() = Some(flag.clone());
        flag
    }

    pub fn clear(&self) {
        *self.current.lock().unwrap() = None;
    }

    pub fn cancel(&self) -> bool {
        if let Some(flag) = self.current.lock().unwrap().as_ref() {
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
    let config = ScanConfig::default();
    let result = run_scan(root_path, provider, &config, Some(tx), Some(cancel_flag.clone()))
        .await
        .map_err(|e| {
            cancel.clear();
            format!("{e:#}")
        })?;

    let _ = forward_task.await;

    let was_cancelled = cancel_flag.load(Ordering::SeqCst);
    cancel.clear();
    let status = if was_cancelled { "cancelled" } else { "completed" };

    match store.save_scan(&result, status) {
        Ok(scan_id) => info!(scan_id = %scan_id, status, "scan persisted"),
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
