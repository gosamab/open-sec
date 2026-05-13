use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use tokio::process::Command;

use super::sandbox::{relativize, resolve_inside};

#[derive(Deserialize)]
struct GitBlameArgs {
    path: String,
    line: u32,
    /// Optional inclusive end line. Defaults to `line`.
    #[serde(default)]
    end_line: Option<u32>,
}

pub async fn git_blame(input: &Value, scan_root: &Path) -> Result<String> {
    let args: GitBlameArgs = serde_json::from_value(input.clone())
        .context("git_blame expects {\"path\":\"...\",\"line\":<int>,\"end_line\":<int?>}")?;
    if args.line == 0 {
        return Err(anyhow!("line must be >= 1"));
    }
    let target = resolve_inside(&args.path, scan_root)?;
    if !target.is_file() {
        return Err(anyhow!("not a file: {}", target.display()));
    }

    if !is_git_repo(scan_root).await {
        return Ok(format!(
            "{}: scan root is not a git repository; blame unavailable.",
            relativize(&target, scan_root).display()
        ));
    }

    let end = args.end_line.unwrap_or(args.line).max(args.line);
    let range = format!("{},{}", args.line, end);

    let output = Command::new("git")
        .arg("-C")
        .arg(scan_root)
        .arg("blame")
        .arg("--date=short")
        .arg("-L")
        .arg(&range)
        .arg("--")
        .arg(&target)
        .output()
        .await
        .context("running git blame")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Ok(format!(
            "{}: git blame failed ({}). {}",
            relativize(&target, scan_root).display(),
            output.status,
            stderr
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let label = relativize(&target, scan_root).display().to_string();
    Ok(format!("{label}:{range}\n{}", stdout.trim_end()))
}

async fn is_git_repo(scan_root: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(scan_root)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn returns_helpful_message_when_not_git_repo() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        fs::write(root.join("a.txt"), "hello\nworld\n").unwrap();
        let out = git_blame(&json!({"path": "a.txt", "line": 1}), &root)
            .await
            .unwrap();
        assert!(out.contains("not a git repository"));
    }
}
