use std::path::PathBuf;

use tracing::{info, instrument};

use crate::config;
use crate::providers::anthropic::AnthropicProvider;
use crate::scanner::detect::{scan_with_tools, DEFAULT_DETECT_MODEL};
use crate::scanner::Finding;

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
