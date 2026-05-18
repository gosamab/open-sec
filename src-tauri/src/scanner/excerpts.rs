//! Extract a code excerpt for a finding's line range. For tree-sitter
//! languages, walks up to the smallest enclosing function/class/etc.
//! Otherwise falls back to a `±N` line window. Computed on demand by the
//! `get_excerpt` IPC when a finding is selected.

use std::path::Path;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Parser};

use super::languages::{lang_for_path, shiki_lang_for_path, Lang};

/// Lines of context above / below the target span when falling back to a
/// plain line-window excerpt.
const CONTEXT_LINES: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExcerptSource {
    /// Enclosing function/class located via tree-sitter.
    EnclosingFunction,
    /// Plain ±N line window — no tree-sitter support for this language, or
    /// no enclosing node was found.
    LineRange,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Excerpt {
    /// Shiki language hint, when known (e.g. "typescript", "rust").
    pub language: Option<String>,
    pub start_line: u32,
    pub end_line: u32,
    pub text: String,
    pub source: ExcerptSource,
}

/// Compute the excerpt for `file` at `line_start..=line_end`.
pub fn extract(file: &Path, line_start: u32, line_end: u32) -> Result<Excerpt> {
    let source = std::fs::read_to_string(file)
        .map_err(|e| anyhow!("read {}: {e}", file.display()))?;
    let shiki = shiki_lang_for_path(file).map(String::from);

    if let Some(lang) = lang_for_path(file) {
        if let Some(ex) = extract_enclosing(&source, lang, line_start, line_end, shiki.clone()) {
            return Ok(ex);
        }
    }
    // Fallback: plain line window.
    Ok(line_window(&source, line_start, line_end, shiki))
}

fn extract_enclosing(
    source: &str,
    lang: Lang,
    line_start: u32,
    line_end: u32,
    shiki: Option<String>,
) -> Option<Excerpt> {
    let ts_lang = lang.tree_sitter_language();
    let mut parser = Parser::new();
    parser.set_language(&ts_lang).ok()?;
    let tree = parser.parse(source, None)?;
    let root = tree.root_node();

    // Convert 1-indexed inclusive line range → byte offsets [start_byte,
    // end_byte). `descendant_for_byte_range` then returns the smallest node
    // whose byte span contains the request.
    let (start_byte, end_byte) = line_range_to_bytes(source, line_start, line_end);
    let target = root.descendant_for_byte_range(start_byte, end_byte)?;

    // Walk up to the smallest enclosing function/class node.
    let enclosing = walk_up_to_enclosing(target, lang)?;
    let bytes = source.as_bytes();
    let text = enclosing.utf8_text(bytes).ok()?.to_string();
    Some(Excerpt {
        language: shiki,
        start_line: (enclosing.start_position().row as u32) + 1,
        end_line: (enclosing.end_position().row as u32) + 1,
        text,
        source: ExcerptSource::EnclosingFunction,
    })
}

/// Byte range covering [line_start..=line_end] inclusive, 1-indexed lines.
fn line_range_to_bytes(source: &str, line_start: u32, line_end: u32) -> (usize, usize) {
    let bytes = source.as_bytes();
    let total = bytes.len();
    if line_start == 0 || total == 0 {
        return (0, total);
    }
    let mut start_byte = 0usize;
    let mut end_byte = total;
    let mut current_line: u32 = 1;
    let mut found_start = line_start == 1;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            current_line += 1;
            if !found_start && current_line == line_start {
                start_byte = i + 1;
                found_start = true;
            }
            if current_line == line_end + 1 {
                end_byte = i;
                break;
            }
        }
        i += 1;
    }
    if !found_start {
        // line_start past EOF
        return (total, total);
    }
    (start_byte, end_byte.min(total))
}

fn walk_up_to_enclosing<'a>(mut node: Node<'a>, lang: Lang) -> Option<Node<'a>> {
    loop {
        if lang.is_enclosing(node.kind()) {
            return Some(node);
        }
        node = node.parent()?;
    }
}

fn line_window(
    source: &str,
    line_start: u32,
    line_end: u32,
    shiki: Option<String>,
) -> Excerpt {
    let lines: Vec<&str> = source.lines().collect();
    let total = lines.len() as u32;
    if total == 0 {
        return Excerpt {
            language: shiki,
            start_line: 1,
            end_line: 1,
            text: String::new(),
            source: ExcerptSource::LineRange,
        };
    }
    let start = line_start.saturating_sub(CONTEXT_LINES).max(1);
    let end = (line_end + CONTEXT_LINES).min(total);
    let slice: Vec<&str> = lines
        .iter()
        .skip((start - 1) as usize)
        .take((end - start + 1) as usize)
        .copied()
        .collect();
    Excerpt {
        language: shiki,
        start_line: start,
        end_line: end,
        text: slice.join("\n"),
        source: ExcerptSource::LineRange,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn enclosing_typescript_function() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("a.ts");
        fs::write(
            &p,
            "function foo(x: number) {\n  return x + 1;\n}\n\nfunction bar() {\n  const y = foo(2);\n  return y;\n}\n",
        )
        .unwrap();
        // Target the body of bar() (lines 6-7).
        let ex = extract(&p, 6, 7).unwrap();
        assert_eq!(ex.source, ExcerptSource::EnclosingFunction);
        assert!(ex.text.contains("function bar()"));
        assert!(ex.text.contains("return y"));
        assert!(!ex.text.contains("function foo"));
        assert_eq!(ex.language.as_deref(), Some("typescript"));
    }

    #[test]
    fn enclosing_python_class() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("a.py");
        fs::write(
            &p,
            "def free():\n    pass\n\nclass A:\n    def m(self):\n        x = 1\n        return x\n",
        )
        .unwrap();
        // Target the body of m (lines 6-7) — expect to walk up to the method
        // (function_definition) not the surrounding class.
        let ex = extract(&p, 6, 7).unwrap();
        assert_eq!(ex.source, ExcerptSource::EnclosingFunction);
        assert!(ex.text.contains("def m(self)"));
        assert!(ex.text.contains("return x"));
    }

    #[test]
    fn enclosing_rust_function() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("a.rs");
        fs::write(
            &p,
            "fn helper() { 1 }\n\nfn main() {\n    let x = helper();\n    println!(\"{}\", x);\n}\n",
        )
        .unwrap();
        let ex = extract(&p, 4, 5).unwrap();
        assert_eq!(ex.source, ExcerptSource::EnclosingFunction);
        assert!(ex.text.contains("fn main()"));
        assert!(!ex.text.contains("fn helper"));
        assert_eq!(ex.language.as_deref(), Some("rust"));
    }

    #[test]
    fn fallback_for_unsupported_extension() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("a.go");
        let src = (1..=20).map(|i| format!("// line {i}")).collect::<Vec<_>>().join("\n");
        fs::write(&p, &src).unwrap();
        let ex = extract(&p, 10, 11).unwrap();
        assert_eq!(ex.source, ExcerptSource::LineRange);
        // ±5 context lines around 10..=11 → 5..=16
        assert_eq!(ex.start_line, 5);
        assert_eq!(ex.end_line, 16);
        assert!(ex.text.contains("// line 10"));
        assert!(ex.text.contains("// line 16"));
        assert!(!ex.text.contains("// line 17"));
        assert_eq!(ex.language.as_deref(), Some("go"));
    }

    #[test]
    fn fallback_when_no_enclosing_node() {
        // Top-level code that isn't inside any function — the tree-sitter
        // pass returns None and we fall back to a line window.
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("a.ts");
        fs::write(&p, "const x = 1;\nconst y = 2;\nconst z = 3;\n").unwrap();
        let ex = extract(&p, 2, 2).unwrap();
        assert_eq!(ex.source, ExcerptSource::LineRange);
    }
}
