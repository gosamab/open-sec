use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

/// Resolve a path argument (absolute or relative to `scan_root`) and verify
/// the canonical result lives inside `scan_root`. Symlinks pointing out of the
/// root are rejected because `canonicalize` follows them and the
/// `starts_with(scan_root)` check fails.
///
/// `scan_root` MUST be canonicalized by the caller exactly once per scan.
pub fn resolve_inside(rel_or_abs: &str, scan_root: &Path) -> Result<PathBuf> {
    let candidate = Path::new(rel_or_abs);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        scan_root.join(candidate)
    };

    let canonical = match joined.canonicalize() {
        Ok(p) => p,
        Err(e) => return Err(anyhow!("canonicalize {}: {}", joined.display(), e)),
    };

    if !canonical.starts_with(scan_root) {
        return Err(anyhow!(
            "path escapes scan root: {} not under {}",
            canonical.display(),
            scan_root.display()
        ));
    }
    Ok(canonical)
}

/// Canonicalize a scan-root once at scan start. Errors if it doesn't exist.
pub fn canonical_root(root: &Path) -> Result<PathBuf> {
    root.canonicalize()
        .with_context(|| format!("canonicalize scan root {}", root.display()))
}

/// Render a path relative to `scan_root` for display (so the model sees stable,
/// project-relative paths, not user-specific absolute paths). Falls back to the
/// absolute path if the strip fails.
pub fn relativize<'a>(path: &'a Path, scan_root: &Path) -> &'a Path {
    path.strip_prefix(scan_root).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        fs::write(root.join("a.txt"), "hello").unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub/b.txt"), "world").unwrap();
        (tmp, root)
    }

    #[test]
    fn accepts_relative_path() {
        let (_tmp, root) = setup();
        let p = resolve_inside("a.txt", &root).unwrap();
        assert!(p.starts_with(&root));
        assert!(p.ends_with("a.txt"));
    }

    #[test]
    fn accepts_nested_relative_path() {
        let (_tmp, root) = setup();
        let p = resolve_inside("sub/b.txt", &root).unwrap();
        assert!(p.ends_with("b.txt"));
    }

    #[test]
    fn accepts_absolute_path_inside_root() {
        let (_tmp, root) = setup();
        let abs = root.join("a.txt");
        let p = resolve_inside(abs.to_str().unwrap(), &root).unwrap();
        assert!(p.ends_with("a.txt"));
    }

    #[test]
    fn rejects_dotdot_traversal() {
        let (_tmp, root) = setup();
        let outside = root.parent().unwrap().to_path_buf();
        fs::write(outside.join("secret.txt"), "leak").unwrap();
        let err = resolve_inside("../secret.txt", &root).unwrap_err();
        assert!(err.to_string().contains("escapes scan root"));
        let _ = fs::remove_file(outside.join("secret.txt"));
    }

    #[test]
    fn rejects_absolute_outside_root() {
        let (_tmp, root) = setup();
        let err = resolve_inside("/etc/hosts", &root).unwrap_err();
        // Either escapes-scan-root or canonicalize-fail depending on platform.
        let msg = err.to_string();
        assert!(msg.contains("escapes") || msg.contains("canonicalize"));
    }
}
