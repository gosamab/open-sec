//! Repo walk + pre-triage skips. Produces the set of files the LLM triage
//! pass will see, plus a parallel `Skipped` list so the funnel is visible in
//! UI later. Everything LLM-touching is downstream — this module is pure I/O
//! + heuristics, fully sync (cheap enough to not need tokio), and unit-tested
//! against tempdirs.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};

/// Hard cap from CLAUDE.md. Files larger than this skip triage entirely.
pub const MAX_FILE_BYTES: u64 = 500 * 1024;

/// Bytes read for binary / minified heuristics.
const SAMPLE_BYTES: usize = 8 * 1024;

/// Minified-heuristic threshold: average line length across the sample.
const MAX_AVG_LINE_LEN: usize = 200;

/// Directory names that always skip, regardless of `.gitignore` state. These
/// are vendor / build / cache directories that almost never contain
/// hand-written first-party code worth scanning.
const EXCLUDED_DIRS: &[&str] = &[
    "node_modules",
    "vendor",
    "dist",
    "build",
    ".next",
    "target",
    "__pycache__",
    ".venv",
    "coverage",
    ".git",
];

/// Extensions whose files we triage. Lowercase, no leading dot.
const ALLOWED_EXTS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "rb", "php", "java", "kt", "swift",
    "cs", "c", "cc", "cpp", "h", "hpp", "m", "mm", "svelte", "vue", "yml", "yaml", "tf", "hcl",
    "sh",
];

/// Exact filenames (case-sensitive) accepted even without a recognized
/// extension.
const ALLOWED_NAMES: &[&str] = &["Dockerfile", "docker-compose.yml", ".env.example"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    /// Absolute path on disk.
    pub path: PathBuf,
    /// Path relative to the scan root (display + IDs).
    pub rel_path: String,
    pub size_bytes: u64,
    pub line_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// Path matched an excluded vendor/build directory.
    ExcludedDir,
    /// Extension not in the allowlist and name not in the exact-name list.
    UnsupportedExt,
    /// File larger than `MAX_FILE_BYTES`.
    TooLarge,
    /// Null byte detected in the first `SAMPLE_BYTES`.
    Binary,
    /// Average line length over the sample exceeded `MAX_AVG_LINE_LEN`.
    Minified,
    /// I/O error while sampling (permission denied, vanished, etc.).
    IoError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skipped {
    pub path: PathBuf,
    pub rel_path: String,
    pub reason: SkipReason,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WalkResult {
    pub candidates: Vec<Candidate>,
    pub skipped: Vec<Skipped>,
}

/// Walk `root`, applying extension/name filters, vendor-dir exclusion, and the
/// per-file content heuristics (size / binary / minified).
///
/// Respects `.gitignore` via the `ignore` crate. The returned paths are
/// canonical absolute paths under the canonicalized `root`.
pub fn walk(root: &Path) -> Result<WalkResult> {
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalizing scan root {}", root.display()))?;

    let mut out = WalkResult::default();

    for entry in WalkBuilder::new(&root)
        .hidden(false) // we want .env.example etc; vendor list handles .git
        .follow_links(false)
        .build()
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }

        let rel = path.strip_prefix(&root).unwrap_or(path);
        let rel_str = rel.to_string_lossy().to_string();

        if is_in_excluded_dir(rel) {
            out.skipped.push(Skipped {
                path: path.to_path_buf(),
                rel_path: rel_str,
                reason: SkipReason::ExcludedDir,
            });
            continue;
        }

        if !is_allowed(path) {
            // Don't even report unsupported extensions in the skip list; that
            // would flood it with every README and lockfile in the tree. Only
            // surface skips for files that *would* have been triaged but were
            // dropped by a content heuristic.
            continue;
        }

        match classify_content(path) {
            Ok(Classification::Keep {
                size_bytes,
                line_count,
            }) => out.candidates.push(Candidate {
                path: path.to_path_buf(),
                rel_path: rel_str,
                size_bytes,
                line_count,
            }),
            Ok(Classification::Skip(reason)) => out.skipped.push(Skipped {
                path: path.to_path_buf(),
                rel_path: rel_str,
                reason,
            }),
            Err(_) => out.skipped.push(Skipped {
                path: path.to_path_buf(),
                rel_path: rel_str,
                reason: SkipReason::IoError,
            }),
        }
    }

    out.candidates.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    out.skipped.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

fn is_in_excluded_dir(rel: &Path) -> bool {
    rel.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        EXCLUDED_DIRS.iter().any(|d| *d == s)
    })
}

fn is_allowed(path: &Path) -> bool {
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if ALLOWED_NAMES.iter().any(|n| *n == name) {
            return true;
        }
    }
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let lower = ext.to_ascii_lowercase();
        return ALLOWED_EXTS.iter().any(|e| *e == lower);
    }
    false
}

enum Classification {
    Keep { size_bytes: u64, line_count: u32 },
    Skip(SkipReason),
}

fn classify_content(path: &Path) -> Result<Classification> {
    let meta = std::fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?;
    let size_bytes = meta.len();
    if size_bytes > MAX_FILE_BYTES {
        return Ok(Classification::Skip(SkipReason::TooLarge));
    }

    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let sample = &bytes[..bytes.len().min(SAMPLE_BYTES)];

    if sample.contains(&0u8) {
        return Ok(Classification::Skip(SkipReason::Binary));
    }

    // Avg line length over the sample. Use byte count of the sample / number
    // of newlines (+1 to avoid div by zero, mirrors the intuition that a
    // newline-less 8KB blob is one very long line).
    let newlines = sample.iter().filter(|&&b| b == b'\n').count();
    let avg_line_len = sample.len() / newlines.max(1);
    if avg_line_len > MAX_AVG_LINE_LEN && newlines > 0 {
        return Ok(Classification::Skip(SkipReason::Minified));
    }

    // A newline-less file shorter than the sample is fine (e.g. short
    // single-line shebang scripts), but a *long* newline-less file is
    // certainly minified.
    if newlines == 0 && sample.len() > MAX_AVG_LINE_LEN {
        return Ok(Classification::Skip(SkipReason::Minified));
    }

    let line_count = bytes.iter().filter(|&&b| b == b'\n').count() as u32 + 1;

    Ok(Classification::Keep {
        size_bytes,
        line_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn touch(dir: &Path, rel: &str, contents: &[u8]) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, contents).unwrap();
    }

    #[test]
    fn keeps_source_files_skips_unsupported() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        touch(root, "src/app.ts", b"const x = 1;\n");
        touch(root, "src/app.py", b"x = 1\n");
        touch(root, "README.md", b"# hi\n"); // unsupported ext — silently ignored
        touch(root, "Dockerfile", b"FROM alpine\n");

        let r = walk(root).unwrap();
        let names: Vec<_> = r.candidates.iter().map(|c| c.rel_path.clone()).collect();
        assert!(names.contains(&"src/app.ts".to_string()));
        assert!(names.contains(&"src/app.py".to_string()));
        assert!(names.contains(&"Dockerfile".to_string()));
        assert!(!names.contains(&"README.md".to_string()));
        // Unsupported ext should NOT show up in the skipped list.
        assert!(!r.skipped.iter().any(|s| s.rel_path == "README.md"));
    }

    #[test]
    fn excludes_vendor_dirs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        touch(root, "node_modules/foo/index.js", b"console.log(1)\n");
        touch(root, "src/handler.js", b"export const h = 1;\n");

        let r = walk(root).unwrap();
        assert_eq!(r.candidates.len(), 1);
        assert_eq!(r.candidates[0].rel_path, "src/handler.js");
        assert!(r
            .skipped
            .iter()
            .any(|s| s.reason == SkipReason::ExcludedDir
                && s.rel_path.starts_with("node_modules/")));
    }

    #[test]
    fn skips_files_over_size_cap() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let big: Vec<u8> = b"const x=1;\n".repeat((MAX_FILE_BYTES as usize / 11) + 100);
        touch(root, "big.ts", &big);
        let r = walk(root).unwrap();
        assert!(r.candidates.is_empty());
        assert_eq!(r.skipped.len(), 1);
        assert_eq!(r.skipped[0].reason, SkipReason::TooLarge);
    }

    #[test]
    fn skips_binary_files() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let mut buf = b"const x = 1;\n".to_vec();
        buf.push(0); // null byte → binary
        buf.extend_from_slice(b"more\n");
        touch(root, "weird.ts", &buf);
        let r = walk(root).unwrap();
        assert!(r.candidates.is_empty());
        assert_eq!(r.skipped[0].reason, SkipReason::Binary);
    }

    #[test]
    fn skips_minified_files() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // 9KB on a single line — average line length blows past 200.
        let long: Vec<u8> = vec![b'a'; 9 * 1024];
        touch(root, "bundle.js", &long);
        let r = walk(root).unwrap();
        assert!(r.candidates.is_empty());
        assert_eq!(r.skipped[0].reason, SkipReason::Minified);
    }

    #[test]
    fn counts_lines() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        touch(root, "a.ts", b"a\nb\nc\n");
        let r = walk(root).unwrap();
        assert_eq!(r.candidates.len(), 1);
        // 3 newlines + 1 = 4 (treats trailing newline as terminator of last
        // line plus empty tail). Good enough for triage budgeting.
        assert_eq!(r.candidates[0].line_count, 4);
        assert_eq!(r.candidates[0].size_bytes, 6);
    }
}
