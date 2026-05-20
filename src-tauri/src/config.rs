use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};

const ANTHROPIC_ENV: &str = "ANTHROPIC_API_KEY";
const ANTHROPIC_FILE: &str = "anthropic-api-key";
const OPENAI_ENV: &str = "OPENAI_API_KEY";
const OPENAI_FILE: &str = "openai-api-key";

// Resolved once at startup from Tauri's app_data_dir. We avoid the macOS
// keychain because unsigned / ad-hoc-signed builds get a fresh code
// signature on every `tauri build`, and the login keychain's ACL is keyed
// off that signature — so a write succeeds in-process but `get_password`
// fails on the next launch. A plain user-only file in the app's data dir
// survives rebuilds and is what every other "no-cert" desktop tool does.
static ANTHROPIC_KEY_PATH: OnceLock<PathBuf> = OnceLock::new();
static OPENAI_KEY_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Called once from `lib.rs::setup` after Tauri resolves `app_data_dir`.
/// Initializes the on-disk credentials paths for both providers.
pub fn init_key_paths(app_data_dir: &Path) {
    let _ = ANTHROPIC_KEY_PATH.set(app_data_dir.join(ANTHROPIC_FILE));
    let _ = OPENAI_KEY_PATH.set(app_data_dir.join(OPENAI_FILE));
}

fn anthropic_path() -> Result<&'static Path> {
    ANTHROPIC_KEY_PATH
        .get()
        .map(|p| p.as_path())
        .ok_or_else(|| anyhow!("credentials path not initialised — init_key_paths was never called"))
}

fn openai_path() -> Result<&'static Path> {
    OPENAI_KEY_PATH
        .get()
        .map(|p| p.as_path())
        .ok_or_else(|| anyhow!("credentials path not initialised — init_key_paths was never called"))
}

/// Load the Anthropic API key from the on-disk credentials file, falling
/// back to the `ANTHROPIC_API_KEY` env var (which dotenvy may have
/// populated from `.env`).
pub fn load_anthropic_key() -> Result<String> {
    load_key(anthropic_path()?, ANTHROPIC_ENV)
}

/// Persist the Anthropic key at <app_data_dir>/anthropic-api-key with 0600
/// perms on Unix. An `Ok(())` here means the next `load_anthropic_key()`
/// will see it.
pub fn store_anthropic_key(key: &str) -> Result<()> {
    store_key(anthropic_path()?, key)
}

pub fn has_anthropic_key() -> bool {
    read_key_file(anthropic_path().ok())
        || std::env::var(ANTHROPIC_ENV)
            .map(|v| !v.is_empty())
            .unwrap_or(false)
}

/// Load the OpenAI API key — same lookup order as Anthropic.
pub fn load_openai_key() -> Result<String> {
    load_key(openai_path()?, OPENAI_ENV)
}

pub fn store_openai_key(key: &str) -> Result<()> {
    store_key(openai_path()?, key)
}

pub fn has_openai_key() -> bool {
    read_key_file(openai_path().ok())
        || std::env::var(OPENAI_ENV)
            .map(|v| !v.is_empty())
            .unwrap_or(false)
}

fn load_key(path: &Path, env: &str) -> Result<String> {
    if let Some(key) = read_key_file_at(path) {
        return Ok(key);
    }
    if let Ok(key) = std::env::var(env) {
        if !key.is_empty() {
            return Ok(key);
        }
    }
    Err(anyhow!(
        "No API key found. Paste one in the app, or set `{env}` in a `.env` file."
    ))
}

fn store_key(path: &Path, key: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(path, key.trim()).with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 0600 {}", path.display()))?;
    }
    Ok(())
}

fn read_key_file(path: Option<&Path>) -> bool {
    path.and_then(read_key_file_at).is_some()
}

fn read_key_file_at(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
