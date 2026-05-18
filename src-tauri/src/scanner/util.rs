//! Helpers shared between scanner stages — parsing strict-JSON model output,
//! collecting text from content blocks, line-numbering source for the model,
//! and resolving a finding's file path against the scan root.

use std::path::{Path, PathBuf};

use crate::providers::ContentBlock;

/// If `file` is an absolute path that already lives inside `root`, return
/// it as-is. Otherwise treat it as a relative path under `root`. Used by
/// verify and patch when feeding the model the focus file.
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

/// Join all `text` blocks in a model response. Tool-use / tool-result blocks
/// are intentionally dropped — callers handle those separately.
pub(super) fn collect_text(content: &[ContentBlock]) -> String {
    let mut out = String::new();
    for block in content {
        if let ContentBlock::Text { text } = block {
            out.push_str(text);
        }
    }
    out
}

/// Find a top-level JSON object inside arbitrary model text. Tolerates the
/// model adding ```json fences or short preamble/postamble despite being
/// told not to.
///
/// Returns the first `{...}` whose first non-whitespace inner byte is a
/// quote — i.e., starts with a quoted JSON key. This avoids picking up
/// numeric-expression braces (e.g. `{16}`) that appear in the model's prose
/// before the real JSON.
pub(super) fn extract_json_object(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    let stripped = strip_fence(trimmed).unwrap_or(trimmed);
    let bytes = stripped.as_bytes();
    let mut search_from = 0;
    while let Some(rel) = stripped[search_from..].find('{') {
        let start = search_from + rel;
        if let Some(rel_close) = matching_close(&stripped[start..]) {
            let end = start + rel_close;
            if looks_like_json_object(&bytes[start + 1..end]) {
                return Some(&stripped[start..=end]);
            }
            // Not a JSON object — advance past this `{` and keep looking.
            search_from = start + 1;
        } else {
            // Unbalanced — no closer exists anywhere; give up.
            return None;
        }
    }
    None
}

/// True if the bytes between `{` and `}` start (after whitespace) with a
/// quoted key. Empty objects `{}` also count — `findings: []` style replies
/// occasionally serialize that way.
fn looks_like_json_object(inner: &[u8]) -> bool {
    let mut i = 0;
    while i < inner.len() && (inner[i] as char).is_whitespace() {
        i += 1;
    }
    i == inner.len() || inner[i] == b'"'
}

fn strip_fence(s: &str) -> Option<&str> {
    let s = s.strip_prefix("```json")?.trim_start();
    s.strip_suffix("```").map(str::trim_end)
}

fn matching_close(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match b {
            b'\\' if in_string => escaped = true,
            b'"' => in_string = !in_string,
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_plain_json() {
        let text = r#"{"findings": []}"#;
        assert_eq!(extract_json_object(text), Some(r#"{"findings": []}"#));
    }

    #[test]
    fn extract_strips_markdown_fence() {
        let text = "```json\n{\"findings\": []}\n```";
        assert_eq!(extract_json_object(text), Some("{\"findings\": []}"));
    }

    #[test]
    fn extract_handles_preamble() {
        let text = "Here's the JSON:\n\n{\"findings\": [{\"a\":1}]}\n\nThanks!";
        assert_eq!(
            extract_json_object(text),
            Some("{\"findings\": [{\"a\":1}]}")
        );
    }

    #[test]
    fn extract_handles_nested_braces_and_strings() {
        let text = r#"{"findings": [{"description": "uses { and } literals"}]}"#;
        assert_eq!(extract_json_object(text), Some(text));
    }

    #[test]
    fn extract_returns_none_when_no_object() {
        assert_eq!(extract_json_object("hello world"), None);
    }

    #[test]
    fn extract_skips_count_brace_in_prose() {
        // The model writes prose like "matches /[a-f]{16}/" and then emits
        // the real JSON. The extractor must skip past `{16}`.
        let text = r#"It validates ids against /^[a-f0-9]{16}$/. Then:
{"findings": [{"a":1}]}"#;
        assert_eq!(
            extract_json_object(text),
            Some(r#"{"findings": [{"a":1}]}"#)
        );
    }

    #[test]
    fn extract_handles_empty_object() {
        assert_eq!(extract_json_object("{}"), Some("{}"));
    }

    #[test]
    fn line_numbers_pad_to_width() {
        let out = with_line_numbers("a\nb\nc");
        assert!(out.starts_with("  1| a\n"));
        assert!(out.contains("  2| b\n"));
        assert!(out.contains("  3| c"));
    }
}
