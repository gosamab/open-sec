pub mod blame;
pub mod fs_tools;
pub mod imports;
pub mod sandbox;
pub mod search;

use std::path::Path;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::providers::{CacheControl, Tool};

pub const READ_FILE: &str = "read_file";
pub const READ_FILE_RANGE: &str = "read_file_range";
pub const GREP: &str = "grep";
pub const FIND_REFERENCES: &str = "find_references";
pub const LIST_DIRECTORY: &str = "list_directory";
pub const LIST_IMPORTS: &str = "list_imports";
pub const GIT_BLAME: &str = "git_blame";

/// JSON-schema tool definitions sent to the model.
/// The LAST tool carries `cache_control` so the entire tool block is cached
/// together with the system prompt (matches the locked 1h-ephemeral policy).
pub fn tool_definitions() -> Vec<Tool> {
    let defs = [
        (
            READ_FILE,
            "Read the full contents of a UTF-8 text file under the scan root. \
             Output is line-numbered. Files larger than 200KB are rejected — \
             use read_file_range for those.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path, relative to scan root or absolute. Must resolve inside scan root."
                    }
                },
                "required": ["path"]
            }),
        ),
        (
            READ_FILE_RANGE,
            "Read a line range from a file. Use for large files or when you only \
             need a specific window of code. Output is line-numbered.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "start": {
                        "type": "integer",
                        "description": "1-indexed first line to read."
                    },
                    "end": {
                        "type": "integer",
                        "description": "1-indexed last line (inclusive). Defaults to start+400."
                    }
                },
                "required": ["path", "start"]
            }),
        ),
        (
            GREP,
            "Regex (or literal) search across files under the scan root. Respects \
             .gitignore. Caps at 200 matches / 40KB of output. Prefer this over \
             reading whole directories.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regex by default; set regex=false for literal." },
                    "path": { "type": "string", "description": "Optional file or directory to scope the search." },
                    "regex": { "type": "boolean", "description": "Treat pattern as regex (default true)." },
                    "ignore_case": { "type": "boolean", "description": "Case-insensitive (default false)." }
                },
                "required": ["pattern"]
            }),
        ),
        (
            FIND_REFERENCES,
            "Find references to an identifier. Like grep but filters out pure \
             comments and lines that are only a string literal. Symbol must be a \
             single identifier (letters / digits / underscores).",
            json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "An identifier name." },
                    "path": { "type": "string", "description": "Optional scoping path." }
                },
                "required": ["symbol"]
            }),
        ),
        (
            LIST_DIRECTORY,
            "List immediate entries of a directory under the scan root (dotfiles \
             hidden, sorted, capped at 200 entries).",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory path. Use \".\" for scan root." }
                },
                "required": ["path"]
            }),
        ),
        (
            LIST_IMPORTS,
            "Parse a file with tree-sitter and return its top-level imports / \
             require() calls / use statements. Supported extensions: \
             .rs .ts .tsx .js .jsx .mjs .cjs .py. Other extensions return a hint.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        ),
        (
            GIT_BLAME,
            "Run `git blame` on a line (or line range) of a file. Reports the \
             scan root is not a git repo gracefully — call once with line=1 to \
             probe if you're unsure.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "line": { "type": "integer", "description": "1-indexed line." },
                    "end_line": { "type": "integer", "description": "Optional inclusive end line." }
                },
                "required": ["path", "line"]
            }),
        ),
    ];

    let last = defs.len() - 1;
    defs.into_iter()
        .enumerate()
        .map(|(i, (name, desc, schema))| Tool {
            name: name.to_string(),
            description: desc.to_string(),
            input_schema: schema,
            cache_control: (i == last).then(CacheControl::ephemeral_1h),
        })
        .collect()
}

pub async fn dispatch(name: &str, input: &Value, scan_root: &Path) -> Result<String> {
    match name {
        READ_FILE => fs_tools::read_file(input, scan_root).await,
        READ_FILE_RANGE => fs_tools::read_file_range(input, scan_root).await,
        GREP => search::grep(input, scan_root).await,
        FIND_REFERENCES => search::find_references(input, scan_root).await,
        LIST_DIRECTORY => fs_tools::list_directory(input, scan_root).await,
        LIST_IMPORTS => imports::list_imports(input, scan_root).await,
        GIT_BLAME => blame::git_blame(input, scan_root).await,
        other => Err(anyhow!("unknown tool: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_control_is_only_on_last_tool() {
        let tools = tool_definitions();
        assert!(tools.len() > 1);
        for (i, t) in tools.iter().enumerate() {
            if i == tools.len() - 1 {
                assert!(t.cache_control.is_some(), "last tool needs cache_control");
            } else {
                assert!(t.cache_control.is_none(), "tool {i} should not be cached");
            }
        }
    }

}
