use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};

const ENV_VAR: &str = "ANTHROPIC_API_KEY";
const KEY_FILENAME: &str = "anthropic-api-key";

// Resolved once at startup from Tauri's app_data_dir. We avoid the macOS
// keychain because unsigned / ad-hoc-signed builds get a fresh code
// signature on every `tauri build`, and the login keychain's ACL is keyed
// off that signature — so a write succeeds in-process but `get_password`
// fails on the next launch. A plain user-only file in the app's data dir
// survives rebuilds and is what every other "no-cert" desktop tool does.
static KEY_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Called once from `lib.rs::setup` after Tauri resolves `app_data_dir`.
pub fn init_key_path(app_data_dir: &Path) {
    let _ = KEY_PATH.set(app_data_dir.join(KEY_FILENAME));
}

fn key_path() -> Result<&'static Path> {
    KEY_PATH
        .get()
        .map(|p| p.as_path())
        .ok_or_else(|| anyhow!("credentials path not initialised — init_key_path was never called"))
}

/// Load the Anthropic API key from the on-disk credentials file, falling
/// back to the `ANTHROPIC_API_KEY` env var (which dotenvy may have
/// populated from `.env`).
pub fn load_anthropic_key() -> Result<String> {
    if let Some(key) = read_key_file() {
        return Ok(key);
    }
    if let Ok(key) = std::env::var(ENV_VAR) {
        if !key.is_empty() {
            return Ok(key);
        }
    }
    Err(anyhow!(
        "No Anthropic API key found. Paste one in the app, or set `{ENV_VAR}` in a `.env` file."
    ))
}

/// Persist the key at <app_data_dir>/anthropic-api-key with 0600 perms on
/// Unix. Round-tripped via a real `fs::write`, so an `Ok(())` here means
/// the next `load_anthropic_key()` will see it.
pub fn store_anthropic_key(key: &str) -> Result<()> {
    let path = key_path()?;
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

pub fn has_anthropic_key() -> bool {
    read_key_file().is_some()
        || std::env::var(ENV_VAR)
            .map(|v| !v.is_empty())
            .unwrap_or(false)
}

fn read_key_file() -> Option<String> {
    let path = key_path().ok()?;
    let raw = fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
