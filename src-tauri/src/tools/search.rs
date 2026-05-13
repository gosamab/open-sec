use std::path::Path;

use anyhow::{anyhow, Context, Result};
use ignore::WalkBuilder;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

use super::sandbox::{relativize, resolve_inside};

const MAX_MATCHES: usize = 200;
const MAX_OUTPUT_BYTES: usize = 40 * 1024;
const FILE_SIZE_LIMIT: u64 = 500 * 1024;

#[derive(Deserialize)]
struct GrepArgs {
    pattern: String,
    /// Optional path (file or directory) to scope the search.
    /// Defaults to scan root.
    #[serde(default)]
    path: Option<String>,
    /// Whether the pattern is a regex. Defaults to true.
    #[serde(default = "default_regex")]
    regex: bool,
    /// Case-insensitive match. Defaults to false.
    #[serde(default)]
    ignore_case: bool,
}

fn default_regex() -> bool {
    true
}

pub async fn grep(input: &Value, scan_root: &Path) -> Result<String> {
    let args: GrepArgs = serde_json::from_value(input.clone())
        .context("grep expects {\"pattern\":\"...\", \"path?\":\"...\", \"regex?\":bool, \"ignore_case?\":bool}")?;
    let pattern = build_regex(&args.pattern, args.regex, args.ignore_case)?;
    let scope = resolve_scope(args.path.as_deref(), scan_root)?;
    let hits = walk_and_match(&pattern, &scope, scan_root, false).await?;
    Ok(format_hits(&hits, &args.pattern, hits.len() >= MAX_MATCHES))
}

#[derive(Deserialize)]
struct FindRefsArgs {
    symbol: String,
    #[serde(default)]
    path: Option<String>,
}

pub async fn find_references(input: &Value, scan_root: &Path) -> Result<String> {
    let args: FindRefsArgs = serde_json::from_value(input.clone())
        .context("find_references expects {\"symbol\":\"...\", \"path?\":\"...\"}")?;
    if !is_identifier(&args.symbol) {
        return Err(anyhow!(
            "find_references symbol must look like an identifier; got {:?}",
            args.symbol
        ));
    }
    let pattern = Regex::new(&format!(r"\b{}\b", regex::escape(&args.symbol)))?;
    let scope = resolve_scope(args.path.as_deref(), scan_root)?;
    let hits = walk_and_match(&pattern, &scope, scan_root, true).await?;
    Ok(format_hits(&hits, &args.symbol, hits.len() >= MAX_MATCHES))
}

fn build_regex(pattern: &str, as_regex: bool, ignore_case: bool) -> Result<Regex> {
    let final_pattern = if as_regex {
        pattern.to_string()
    } else {
        regex::escape(pattern)
    };
    let with_flags = if ignore_case {
        format!("(?i){final_pattern}")
    } else {
        final_pattern
    };
    Regex::new(&with_flags).with_context(|| format!("invalid regex: {pattern}"))
}

fn resolve_scope(path: Option<&str>, scan_root: &Path) -> Result<std::path::PathBuf> {
    match path {
        Some(p) => resolve_inside(p, scan_root),
        None => Ok(scan_root.to_path_buf()),
    }
}

struct Hit {
    file: String,
    line: u32,
    text: String,
}

async fn walk_and_match(
    pattern: &Regex,
    scope: &Path,
    scan_root: &Path,
    strip_strings_and_comments: bool,
) -> Result<Vec<Hit>> {
    let pattern = pattern.clone();
    let scope = scope.to_path_buf();
    let scan_root = scan_root.to_path_buf();

    tokio::task::spawn_blocking(move || -> Result<Vec<Hit>> {
        let mut hits = Vec::new();
        let mut bytes_out = 0usize;

        let walker = WalkBuilder::new(&scope)
            .hidden(false)
            .git_ignore(true)
            .git_exclude(true)
            .build();

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.path();
            let meta = match path.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.len() > FILE_SIZE_LIMIT {
                continue;
            }
            let bytes = match std::fs::read(path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            if bytes.iter().take(8192).any(|b| *b == 0) {
                continue;
            }
            let text = match std::str::from_utf8(&bytes) {
                Ok(s) => s,
                Err(_) => continue,
            };

            for (idx, line) in text.lines().enumerate() {
                if !pattern.is_match(line) {
                    continue;
                }
                if strip_strings_and_comments && !passes_reference_filter(line, &pattern) {
                    continue;
                }
                let rel = relativize(path, &scan_root).display().to_string();
                let snippet = trim_line(line);
                bytes_out += rel.len() + snippet.len() + 16;
                hits.push(Hit {
                    file: rel,
                    line: (idx + 1) as u32,
                    text: snippet,
                });
                if hits.len() >= MAX_MATCHES || bytes_out >= MAX_OUTPUT_BYTES {
                    return Ok(hits);
                }
            }
        }
        Ok(hits)
    })
    .await
    .map_err(|e| anyhow!("walk task panicked: {e}"))?
}

/// Heuristic for `find_references`:
/// - skip pure-comment lines (//, #, *)
/// - skip lines where the pattern only matches inside a string literal
///   (we strip quoted segments and re-test the pattern against the residue)
fn passes_reference_filter(line: &str, pattern: &Regex) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with('*') {
        return false;
    }
    let stripped = strip_quoted_segments(trimmed);
    pattern.is_match(&stripped)
}

fn strip_quoted_segments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' || c == '\'' || c == '`' {
            let quote = c;
            while let Some(next) = chars.next() {
                if next == '\\' {
                    chars.next();
                    continue;
                }
                if next == quote {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn is_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn trim_line(line: &str) -> String {
    const LINE_CAP: usize = 240;
    if line.len() <= LINE_CAP {
        line.to_string()
    } else {
        format!("{}…", &line[..LINE_CAP])
    }
}

fn format_hits(hits: &[Hit], query: &str, truncated: bool) -> String {
    if hits.is_empty() {
        return format!("no matches for {query:?}");
    }
    let mut out = format!("{} match(es) for {query:?}:\n", hits.len());
    for h in hits {
        use std::fmt::Write;
        let _ = writeln!(&mut out, "{}:{}: {}", h.file, h.line, h.text);
    }
    if truncated {
        out.push_str(&format!(
            "(truncated; {} matches or {}KB output cap hit)\n",
            MAX_MATCHES,
            MAX_OUTPUT_BYTES / 1024
        ));
    }
    out
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
        fs::write(
            root.join("a.ts"),
            "import { foo } from './b';\nfoo();\n// foo in a comment\nconst s = \"foo only\";\n",
        )
        .unwrap();
        fs::write(root.join("b.ts"), "export function foo() {}\n").unwrap();
        (tmp, root)
    }

    #[tokio::test]
    async fn grep_finds_literal() {
        let (_tmp, root) = setup();
        let out = grep(
            &json!({"pattern": "foo", "regex": false}),
            &root,
        )
        .await
        .unwrap();
        assert!(out.contains("a.ts:1:"));
        assert!(out.contains("a.ts:2:"));
        assert!(out.contains("b.ts:1:"));
    }

    #[tokio::test]
    async fn find_references_skips_comment_and_string_only_lines() {
        let (_tmp, root) = setup();
        let out = find_references(&json!({"symbol": "foo"}), &root)
            .await
            .unwrap();
        // a.ts:1 (import) and a.ts:2 (call) should be kept; line 3 (comment),
        // line 4 (string-only) should be filtered.
        assert!(out.contains("a.ts:1:"));
        assert!(out.contains("a.ts:2:"));
        assert!(!out.contains("a.ts:3:"));
        assert!(!out.contains("a.ts:4:"));
        assert!(out.contains("b.ts:1:"));
    }

    #[test]
    fn identifier_check_rejects_garbage() {
        assert!(is_identifier("foo"));
        assert!(is_identifier("_bar"));
        assert!(is_identifier("foo_bar2"));
        assert!(!is_identifier(""));
        assert!(!is_identifier("foo bar"));
        assert!(!is_identifier("2foo"));
        assert!(!is_identifier("foo()"));
    }
}
