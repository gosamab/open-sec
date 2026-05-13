use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::Value;

use super::sandbox::{relativize, resolve_inside};

const FULL_READ_MAX_BYTES: usize = 200 * 1024;
const DEFAULT_RANGE_LINES: u32 = 400;
const LIST_DIR_CAP: usize = 200;

#[derive(Deserialize)]
struct ReadFileArgs {
    path: String,
}

pub async fn read_file(input: &Value, scan_root: &Path) -> Result<String> {
    let args: ReadFileArgs = serde_json::from_value(input.clone())
        .context("read_file expects {\"path\": \"<path>\"}")?;
    let target = resolve_inside(&args.path, scan_root)?;
    if !target.is_file() {
        return Err(anyhow!("not a file: {}", target.display()));
    }
    let bytes = tokio::fs::read(&target).await?;
    if bytes.len() > FULL_READ_MAX_BYTES {
        return Err(anyhow!(
            "file too large ({} bytes; {}KB cap). Use read_file_range instead.",
            bytes.len(),
            FULL_READ_MAX_BYTES / 1024
        ));
    }
    if has_null_bytes(&bytes) {
        return Err(anyhow!("file appears to be binary"));
    }
    let text = String::from_utf8_lossy(&bytes);
    Ok(format_with_lines(
        relativize(&target, scan_root).display().to_string(),
        &text,
        1,
    ))
}

#[derive(Deserialize)]
struct ReadFileRangeArgs {
    path: String,
    start: u32,
    #[serde(default)]
    end: Option<u32>,
}

pub async fn read_file_range(input: &Value, scan_root: &Path) -> Result<String> {
    let args: ReadFileRangeArgs = serde_json::from_value(input.clone())
        .context("read_file_range expects {\"path\":\"...\",\"start\":<int>,\"end\":<int?>}")?;
    if args.start == 0 {
        return Err(anyhow!("start must be >= 1 (1-indexed)"));
    }
    let target = resolve_inside(&args.path, scan_root)?;
    if !target.is_file() {
        return Err(anyhow!("not a file: {}", target.display()));
    }
    let bytes = tokio::fs::read(&target).await?;
    if has_null_bytes(&bytes) {
        return Err(anyhow!("file appears to be binary"));
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();

    let start_idx = (args.start as usize).saturating_sub(1);
    if start_idx >= lines.len() {
        return Err(anyhow!(
            "start {} is past EOF ({} lines)",
            args.start,
            lines.len()
        ));
    }
    let end_excl = match args.end {
        Some(e) => (e as usize).min(lines.len()),
        None => (start_idx + DEFAULT_RANGE_LINES as usize).min(lines.len()),
    };
    let slice = lines[start_idx..end_excl].join("\n");
    Ok(format_with_lines(
        relativize(&target, scan_root).display().to_string(),
        &slice,
        args.start,
    ))
}

#[derive(Deserialize)]
struct ListDirArgs {
    path: String,
}

pub async fn list_directory(input: &Value, scan_root: &Path) -> Result<String> {
    let args: ListDirArgs = serde_json::from_value(input.clone())
        .context("list_directory expects {\"path\":\"...\"}")?;
    let target = resolve_inside(&args.path, scan_root)?;
    if !target.is_dir() {
        return Err(anyhow!("not a directory: {}", target.display()));
    }
    let mut entries = tokio::fs::read_dir(&target).await?;
    let mut rows: Vec<(String, &'static str)> = Vec::new();
    while let Some(ent) = entries.next_entry().await? {
        let name = ent.file_name().to_string_lossy().to_string();
        // Skip dotfiles and common noise so we don't drown the model.
        if name.starts_with('.') {
            continue;
        }
        let ty = ent.file_type().await?;
        let kind = if ty.is_dir() {
            "dir"
        } else if ty.is_symlink() {
            "link"
        } else {
            "file"
        };
        rows.push((name, kind));
    }
    rows.sort();
    let truncated = rows.len() > LIST_DIR_CAP;
    rows.truncate(LIST_DIR_CAP);

    let mut out = String::new();
    let rel = relativize(&target, scan_root).display().to_string();
    out.push_str(&format!("{}/\n", if rel.is_empty() { "." } else { &rel }));
    for (name, kind) in &rows {
        out.push_str(&format!("  {kind:4} {name}\n"));
    }
    if truncated {
        out.push_str(&format!("  … ({} entries; showing first {})\n", LIST_DIR_CAP, rows.len()));
    }
    Ok(out)
}

fn format_with_lines(label: String, text: &str, start_line: u32) -> String {
    let total_lines = text.lines().count() as u32;
    let end_line = start_line + total_lines.saturating_sub(1);
    let width = end_line.to_string().len().max(3);
    let mut out = format!("{label}:{start_line}-{end_line}\n");
    for (i, line) in text.lines().enumerate() {
        use std::fmt::Write;
        let _ = writeln!(
            &mut out,
            "{:>width$}| {}",
            start_line + i as u32,
            line,
            width = width
        );
    }
    out
}

fn has_null_bytes(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|b| *b == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;

    fn setup() -> (TempDir, std::path::PathBuf) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        fs::write(root.join("a.ts"), "line1\nline2\nline3\nline4\nline5\n").unwrap();
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        (tmp, root)
    }

    #[tokio::test]
    async fn read_file_basic() {
        let (_tmp, root) = setup();
        let out = read_file(&json!({"path": "a.ts"}), &root).await.unwrap();
        assert!(out.contains("a.ts:1-5"));
        assert!(out.contains("  1| line1"));
        assert!(out.contains("  5| line5"));
    }

    #[tokio::test]
    async fn read_file_range_basic() {
        let (_tmp, root) = setup();
        let out =
            read_file_range(&json!({"path": "a.ts", "start": 2, "end": 4}), &root)
                .await
                .unwrap();
        assert!(out.contains("a.ts:2-4"));
        assert!(out.contains("  2| line2"));
        assert!(out.contains("  4| line4"));
        assert!(!out.contains("  1| line1"));
        assert!(!out.contains("  5| line5"));
    }

    #[tokio::test]
    async fn read_file_rejects_dotdot() {
        let (_tmp, root) = setup();
        let err = read_file(&json!({"path": "../etc/hosts"}), &root)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("escapes") || msg.contains("canonicalize"));
    }

    #[tokio::test]
    async fn list_directory_lists_root() {
        let (_tmp, root) = setup();
        let out = list_directory(&json!({"path": "."}), &root).await.unwrap();
        assert!(out.contains("file a.ts"));
        assert!(out.contains("dir  src"));
    }
}
