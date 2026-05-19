//! Helpers shared between scanner stages.

use std::path::{Path, PathBuf};

/// If `file` is an absolute path, return it as-is; otherwise treat it as a
/// relative path under `root`. Used by verify and patch when feeding the
/// model the focus file.
pub(super) fn resolve_focus_path(root: &Path, file: &str) -> PathBuf {
    let p = PathBuf::from(file);
    if p.is_absolute() {
        p
    } else {
        root.join(p)
    }
}

/// Prefix every line of `source` with its 1-based number so the model can
/// emit precise line refs without us having to teach it our line policy.
pub(super) fn with_line_numbers(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let width = lines.len().to_string().len().max(3);
    let mut out = String::with_capacity(source.len() + lines.len() * (width + 3));
    for (i, line) in lines.iter().enumerate() {
        use std::fmt::Write;
        let _ = writeln!(&mut out, "{:>width$}| {}", i + 1, line, width = width);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_numbers_pad_to_width() {
        let out = with_line_numbers("a\nb\nc");
        assert!(out.starts_with("  1| a\n"));
        assert!(out.contains("  2| b\n"));
        assert!(out.contains("  3| c"));
    }
}
