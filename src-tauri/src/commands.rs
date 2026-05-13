use std::path::PathBuf;

use tracing::{info, instrument};

use crate::config;
use crate::providers::anthropic::AnthropicProvider;
use crate::scanner::detect::{scan_single_file, DEFAULT_DETECT_MODEL};
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

#[tauri::command]
#[instrument(skip_all, fields(path = %path))]
pub async fn scan_file(path: String) -> Result<Vec<Finding>, String> {
    let pb = PathBuf::from(&path);
    if !pb.is_file() {
        return Err(format!("not a file: {path}"));
    }
    let source = tokio::fs::read_to_string(&pb)
        .await
        .map_err(|e| format!("read {}: {e}", pb.display()))?;

    let api_key = config::load_anthropic_key().map_err(|e| e.to_string())?;
    let provider = AnthropicProvider::new(api_key).map_err(|e| e.to_string())?;

    scan_single_file(&pb, &source, &provider, DEFAULT_DETECT_MODEL)
        .await
        .map_err(|e| format!("{e:#}"))
}
