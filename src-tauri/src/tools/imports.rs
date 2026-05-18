use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

use super::sandbox::{relativize, resolve_inside};
use crate::scanner::languages::{lang_for_path, Lang};

#[derive(Deserialize)]
struct ListImportsArgs {
    path: String,
}

pub async fn list_imports(input: &Value, scan_root: &Path) -> Result<String> {
    let args: ListImportsArgs =
        serde_json::from_value(input.clone()).context("list_imports expects {\"path\":\"...\"}")?;
    let target = resolve_inside(&args.path, scan_root)?;
    if !target.is_file() {
        return Err(anyhow!("not a file: {}", target.display()));
    }
    let source = tokio::fs::read_to_string(&target).await?;
    let label = relativize(&target, scan_root).display().to_string();

    let imports = match lang_for_path(&target) {
        Some(lang) => extract_imports(&source, lang)?,
        None => {
            return Ok(format!(
                "{label}: list_imports does not support this file extension yet; use grep for `import`/`require`/`use`."
            ));
        }
    };

    if imports.is_empty() {
        return Ok(format!("{label}: no imports found"));
    }
    let mut out = format!("{label}: {} import(s)\n", imports.len());
    for line in imports {
        out.push_str(&format!("  {line}\n"));
    }
    Ok(out)
}

fn extract_imports(source: &str, lang: Lang) -> Result<Vec<String>> {
    let ts_lang = lang.tree_sitter_language();
    let mut parser = Parser::new();
    parser
        .set_language(&ts_lang)
        .map_err(|e| anyhow!("set_language: {e}"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("tree-sitter parse failed"))?;

    let query = Query::new(&ts_lang, lang.imports_query())
        .map_err(|e| anyhow!("query compile: {e}"))?;
    let mut cursor = QueryCursor::new();
    let mut out = Vec::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

    let import_capture = query
        .capture_index_for_name("import")
        .ok_or_else(|| anyhow!("missing @import capture"))?;

    while let Some(m) = matches.next() {
        for cap in m.captures.iter().filter(|c| c.index == import_capture) {
            let node = cap.node;
            if let Ok(text) = node.utf8_text(source.as_bytes()) {
                let line = node.start_position().row + 1;
                let snippet = collapse_whitespace(text);
                out.push(format!("L{line}: {snippet}"));
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
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
            "import { foo } from './b';\nimport * as x from 'lodash';\nconst r = require('fs');\n",
        )
        .unwrap();
        fs::write(
            root.join("a.py"),
            "import os\nfrom flask import Flask, request\nimport sys\n",
        )
        .unwrap();
        fs::write(
            root.join("a.rs"),
            "use std::path::Path;\nuse anyhow::{Context, Result};\n",
        )
        .unwrap();
        fs::write(
            root.join("a.dart"),
            "import 'package:flutter/material.dart';\nimport 'dart:io';\n",
        )
        .unwrap();
        fs::write(
            root.join("A.java"),
            "package com.example;\nimport java.util.List;\nimport java.io.*;\n",
        )
        .unwrap();
        fs::write(
            root.join("a.cs"),
            "using System;\nusing System.Collections.Generic;\nnamespace Foo {}\n",
        )
        .unwrap();
        fs::write(
            root.join("a.html"),
            "<html><head><script src=\"app.js\"></script><link href=\"main.css\" rel=\"stylesheet\"></head></html>\n",
        )
        .unwrap();
        fs::write(root.join("a.txt"), "no language").unwrap();
        (tmp, root)
    }

    #[tokio::test]
    async fn ts_imports() {
        let (_tmp, root) = setup();
        let out = list_imports(&json!({"path": "a.ts"}), &root).await.unwrap();
        assert!(out.contains("import { foo } from './b'"));
        assert!(out.contains("import * as x from 'lodash'"));
        assert!(out.contains("require('fs')") || out.contains("require ( 'fs' )"));
    }

    #[tokio::test]
    async fn python_imports() {
        let (_tmp, root) = setup();
        let out = list_imports(&json!({"path": "a.py"}), &root).await.unwrap();
        assert!(out.contains("import os"));
        assert!(out.contains("from flask import Flask, request"));
        assert!(out.contains("import sys"));
    }

    #[tokio::test]
    async fn rust_imports() {
        let (_tmp, root) = setup();
        let out = list_imports(&json!({"path": "a.rs"}), &root).await.unwrap();
        assert!(out.contains("use std::path::Path;"));
        assert!(out.contains("use anyhow::{Context, Result};"));
    }

    #[tokio::test]
    async fn dart_imports() {
        let (_tmp, root) = setup();
        let out = list_imports(&json!({"path": "a.dart"}), &root).await.unwrap();
        assert!(out.contains("package:flutter/material.dart"));
        assert!(out.contains("dart:io"));
    }

    #[tokio::test]
    async fn java_imports() {
        let (_tmp, root) = setup();
        let out = list_imports(&json!({"path": "A.java"}), &root).await.unwrap();
        assert!(out.contains("java.util.List"));
        assert!(out.contains("java.io"));
    }

    #[tokio::test]
    async fn csharp_imports() {
        let (_tmp, root) = setup();
        let out = list_imports(&json!({"path": "a.cs"}), &root).await.unwrap();
        assert!(out.contains("using System;"));
        assert!(out.contains("System.Collections.Generic"));
    }

    #[tokio::test]
    async fn html_imports() {
        let (_tmp, root) = setup();
        let out = list_imports(&json!({"path": "a.html"}), &root).await.unwrap();
        assert!(out.to_lowercase().contains("script") || out.contains("src=\"app.js\""));
        assert!(out.to_lowercase().contains("link") || out.contains("href=\"main.css\""));
    }

    #[tokio::test]
    async fn unsupported_extension_returns_hint() {
        let (_tmp, root) = setup();
        let out = list_imports(&json!({"path": "a.txt"}), &root).await.unwrap();
        assert!(out.contains("does not support this file extension"));
    }
}
