use anyhow::{anyhow, Result};

const KEYRING_SERVICE: &str = "open-sec";
const KEYRING_ACCOUNT: &str = "anthropic";
const ENV_VAR: &str = "ANTHROPIC_API_KEY";

/// Load the Anthropic API key from the OS keychain, falling back to the
/// `ANTHROPIC_API_KEY` env var (which dotenvy may have populated from `.env`).
pub fn load_anthropic_key() -> Result<String> {
    if let Some(key) = keychain_get() {
        return Ok(key);
    }
    if let Ok(key) = std::env::var(ENV_VAR) {
        if !key.is_empty() {
            return Ok(key);
        }
    }
    Err(anyhow!(
        "No Anthropic API key found. Set `{ENV_VAR}` in a `.env` file or store one in the OS keychain (open-sec/anthropic)."
    ))
}

pub fn store_anthropic_key(key: &str) -> Result<()> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)?;
    entry.set_password(key)?;
    Ok(())
}

pub fn has_anthropic_key() -> bool {
    keychain_get().is_some()
        || std::env::var(ENV_VAR)
            .map(|v| !v.is_empty())
            .unwrap_or(false)
}

fn keychain_get() -> Option<String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).ok()?;
    entry.get_password().ok().filter(|s| !s.is_empty())
}
