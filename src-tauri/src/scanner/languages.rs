//! Single source of truth for every programming language the scanner knows
//! about. Each entry says:
//!   - which file extensions count as that language (for the ingest walker)
//!   - the Shiki language id (for syntax highlighting in the UI)
//!   - whether we have a tree-sitter grammar wired up (for the
//!     enclosing-function excerpt + `list_imports` tool)
//!
//! Ingest, excerpts, and the `list_imports` tool all consult this table —
//! add a language once here and every layer picks it up.

use std::path::Path;
use tree_sitter::Language;

/// Tree-sitter grammar identity. A language without a grammar falls back to
/// the ±N-line excerpt window and a "use grep" hint from `list_imports`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    JavaScript,
    Typescript,
    Tsx,
    Python,
    Dart,
    Java,
    CSharp,
    Html,
}

impl Lang {
    pub fn tree_sitter_language(self) -> Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::Typescript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::Dart => tree_sitter_dart::LANGUAGE.into(),
            Lang::Java => tree_sitter_java::LANGUAGE.into(),
            Lang::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            Lang::Html => tree_sitter_html::LANGUAGE.into(),
        }
    }

    /// Node kinds that count as an "enclosing function/class/etc." when
    /// climbing up from a finding's span. Conservative — we'd rather walk
    /// further up than not at all.
    pub fn is_enclosing(self, kind: &str) -> bool {
        match self {
            Lang::Rust => matches!(
                kind,
                "function_item"
                    | "function_signature_item"
                    | "impl_item"
                    | "trait_item"
                    | "struct_item"
                    | "enum_item"
                    | "mod_item"
                    | "closure_expression"
            ),
            Lang::JavaScript | Lang::Typescript | Lang::Tsx => matches!(
                kind,
                "function_declaration"
                    | "function_expression"
                    | "function"
                    | "generator_function_declaration"
                    | "generator_function"
                    | "arrow_function"
                    | "method_definition"
                    | "method_signature"
                    | "class_declaration"
                    | "class_expression"
            ),
            Lang::Python => matches!(kind, "function_definition" | "class_definition"),
            Lang::Dart => matches!(
                kind,
                "function_signature"
                    | "function_body"
                    | "method_signature"
                    | "getter_signature"
                    | "setter_signature"
                    | "constructor_signature"
                    | "class_definition"
                    | "mixin_declaration"
                    | "extension_declaration"
                    | "function_expression"
            ),
            Lang::Java => matches!(
                kind,
                "method_declaration"
                    | "constructor_declaration"
                    | "class_declaration"
                    | "interface_declaration"
                    | "enum_declaration"
                    | "annotation_type_declaration"
                    | "record_declaration"
                    | "lambda_expression"
            ),
            Lang::CSharp => matches!(
                kind,
                "method_declaration"
                    | "constructor_declaration"
                    | "destructor_declaration"
                    | "local_function_statement"
                    | "class_declaration"
                    | "interface_declaration"
                    | "struct_declaration"
                    | "record_declaration"
                    | "enum_declaration"
                    | "delegate_declaration"
                    | "namespace_declaration"
                    | "lambda_expression"
            ),
            // HTML doesn't really have "functions" — only walk up to
            // <script>/<style> blocks; anything else falls back to the
            // line window.
            Lang::Html => matches!(kind, "script_element" | "style_element"),
        }
    }

    /// tree-sitter query that captures the "imports" surface for this
    /// language. Each match is reported by `list_imports`.
    pub fn imports_query(self) -> &'static str {
        match self {
            Lang::Rust => "(use_declaration) @import",
            // Covers ES import + CommonJS require + dynamic import().
            Lang::JavaScript | Lang::Typescript | Lang::Tsx => {
                r#"
                (import_statement) @import
                (call_expression
                    function: (identifier) @fn
                    (#eq? @fn "require")) @import
                "#
            }
            // Covers `import foo` and `from foo import bar`.
            Lang::Python => {
                r#"
                (import_statement) @import
                (import_from_statement) @import
                "#
            }
            // `import 'package:foo/bar.dart';` plus `part` and `export`.
            Lang::Dart => "(import_or_export) @import",
            Lang::Java => "(import_declaration) @import",
            Lang::CSharp => "(using_directive) @import",
            // External references: any tag with src/href, plus <script>.
            Lang::Html => {
                r#"
                (script_element) @import
                (start_tag
                    (attribute (attribute_name) @attr)
                    (#match? @attr "^(src|href)$")) @import
                (self_closing_tag
                    (attribute (attribute_name) @attr2)
                    (#match? @attr2 "^(src|href)$")) @import
                "#
            }
        }
    }
}

struct LangSpec {
    /// File extensions (lowercase, no leading dot). The first one is canonical
    /// and used when reverse-mapping for log lines / display.
    extensions: &'static [&'static str],
    /// Shiki language id used by the frontend syntax highlighter. Pick the
    /// closest match if Shiki doesn't have an exact one.
    shiki: &'static str,
    /// Some(Lang) if we have a tree-sitter grammar; None means the language
    /// only gets ingest + Shiki highlighting, no structural features.
    tree_sitter: Option<Lang>,
}

/// Every language the scanner accepts. Add a new language here and ingest,
/// excerpts, imports, and the UI all pick it up. Keep the order roughly
/// "most common first" so debug prints / future log lines read naturally.
const LANGUAGES: &[LangSpec] = &[
    // ---- Languages with full tree-sitter support ----
    LangSpec {
        extensions: &["rs"],
        shiki: "rust",
        tree_sitter: Some(Lang::Rust),
    },
    LangSpec {
        extensions: &["ts", "mts", "cts"],
        shiki: "typescript",
        tree_sitter: Some(Lang::Typescript),
    },
    LangSpec {
        extensions: &["tsx"],
        shiki: "tsx",
        tree_sitter: Some(Lang::Tsx),
    },
    LangSpec {
        extensions: &["js", "mjs", "cjs"],
        shiki: "javascript",
        tree_sitter: Some(Lang::JavaScript),
    },
    LangSpec {
        extensions: &["jsx"],
        shiki: "jsx",
        tree_sitter: Some(Lang::JavaScript),
    },
    LangSpec {
        extensions: &["py"],
        shiki: "python",
        tree_sitter: Some(Lang::Python),
    },
    LangSpec {
        extensions: &["dart"],
        shiki: "dart",
        tree_sitter: Some(Lang::Dart),
    },
    LangSpec {
        extensions: &["java"],
        shiki: "java",
        tree_sitter: Some(Lang::Java),
    },
    LangSpec {
        extensions: &["cs"],
        shiki: "csharp",
        tree_sitter: Some(Lang::CSharp),
    },
    LangSpec {
        extensions: &["html", "htm"],
        shiki: "html",
        tree_sitter: Some(Lang::Html),
    },
    // ---- Languages with ingest + Shiki only (no tree-sitter grammar) ----
    LangSpec {
        extensions: &["go"],
        shiki: "go",
        tree_sitter: None,
    },
    LangSpec {
        extensions: &["rb"],
        shiki: "ruby",
        tree_sitter: None,
    },
    LangSpec {
        extensions: &["php"],
        shiki: "php",
        tree_sitter: None,
    },
    LangSpec {
        extensions: &["kt"],
        shiki: "kotlin",
        tree_sitter: None,
    },
    LangSpec {
        extensions: &["swift"],
        shiki: "swift",
        tree_sitter: None,
    },
    LangSpec {
        extensions: &["c", "h"],
        shiki: "c",
        tree_sitter: None,
    },
    LangSpec {
        extensions: &["cc", "cpp", "cxx", "hpp"],
        shiki: "cpp",
        tree_sitter: None,
    },
    LangSpec {
        extensions: &["m", "mm"],
        shiki: "objective-c",
        tree_sitter: None,
    },
    LangSpec {
        extensions: &["svelte"],
        shiki: "svelte",
        tree_sitter: None,
    },
    LangSpec {
        extensions: &["vue"],
        shiki: "vue",
        tree_sitter: None,
    },
    LangSpec {
        extensions: &["yml", "yaml"],
        shiki: "yaml",
        tree_sitter: None,
    },
    LangSpec {
        extensions: &["tf", "hcl"],
        shiki: "hcl",
        tree_sitter: None,
    },
    LangSpec {
        extensions: &["sh"],
        shiki: "bash",
        tree_sitter: None,
    },
];

/// Exact filenames (case-sensitive) accepted even without a recognized
/// extension. Mostly infrastructure config files.
pub const ALLOWED_NAMES: &[&str] = &["Dockerfile", "docker-compose.yml", ".env.example"];

fn extension_of(path: &Path) -> Option<String> {
    path.extension()?.to_str().map(|s| s.to_ascii_lowercase())
}

fn spec_for_ext(ext: &str) -> Option<&'static LangSpec> {
    LANGUAGES
        .iter()
        .find(|spec| spec.extensions.iter().any(|e| *e == ext))
}

/// `true` if the file's extension (or its bare name) is in the scanner's
/// allowlist. Used by the ingest walker.
pub fn is_scannable_path(path: &Path) -> bool {
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if ALLOWED_NAMES.iter().any(|n| *n == name) {
            return true;
        }
    }
    extension_of(path)
        .as_deref()
        .map(|ext| spec_for_ext(ext).is_some())
        .unwrap_or(false)
}

/// Tree-sitter language for this file, if any. Files without a grammar
/// return `None` and the caller falls back appropriately.
pub fn lang_for_path(path: &Path) -> Option<Lang> {
    let ext = extension_of(path)?;
    spec_for_ext(&ext)?.tree_sitter
}

/// Shiki language id for this file, if any. Returns `None` for unrecognized
/// extensions; the UI falls back to plain text.
pub fn shiki_lang_for_path(path: &Path) -> Option<&'static str> {
    let ext = extension_of(path)?;
    Some(spec_for_ext(&ext)?.shiki)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn html_dart_csharp_java_are_scannable() {
        for ext in &["html", "htm", "dart", "cs", "java"] {
            let p = PathBuf::from(format!("x.{ext}"));
            assert!(is_scannable_path(&p), "{ext} should be scannable");
            assert!(lang_for_path(&p).is_some(), "{ext} should have a grammar");
        }
    }

    #[test]
    fn go_kotlin_swift_have_no_grammar_but_are_scannable() {
        for ext in &["go", "kt", "swift", "rb", "php"] {
            let p = PathBuf::from(format!("x.{ext}"));
            assert!(is_scannable_path(&p), "{ext} should be scannable");
            assert!(lang_for_path(&p).is_none(), "{ext} should NOT have a grammar");
            assert!(shiki_lang_for_path(&p).is_some(), "{ext} should have a Shiki name");
        }
    }

    #[test]
    fn dockerfile_passes_without_extension() {
        assert!(is_scannable_path(&PathBuf::from("Dockerfile")));
    }

    #[test]
    fn unrecognized_extension_is_skipped() {
        assert!(!is_scannable_path(&PathBuf::from("notes.txt")));
        assert!(!is_scannable_path(&PathBuf::from("README.md")));
    }
}
