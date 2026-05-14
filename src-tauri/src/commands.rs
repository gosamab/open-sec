use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;
use tracing::{info, instrument, warn};

use crate::config;
use crate::providers::anthropic::AnthropicProvider;
use crate::providers::Provider;
use crate::scanner::detect::{scan_with_tools, DEFAULT_DETECT_MODEL};
use crate::scanner::orchestrate::{run_scan, ScanConfig, ScanEvent, ScanResult};
use crate::scanner::Finding;
use crate::store::{ScanGroup, Store};

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
#[tauri::command]
#[instrument(skip(app, store), fields(root = %root))]
pub async fn run_pipeline(
    app: AppHandle,
    store: State<'_, Store>,
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

    let config = ScanConfig::default();
    let result = run_scan(root_path, provider, &config, Some(tx))
        .await
        .map_err(|e| format!("{e:#}"))?;

    let _ = forward_task.await;

    // Persist asynchronously of the IPC response — failures here shouldn't
    // poison the scan result the user just got, but should be logged.
    match store.save_scan(&result, "completed") {
        Ok(scan_id) => info!(scan_id = %scan_id, "scan persisted"),
        Err(e) => warn!(error = %format!("{e:#}"), "failed to persist scan"),
    }

    Ok(result)
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
