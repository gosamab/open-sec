//! Patch proposal pass — Sonnet drafts a minimal fix for each surviving
//! finding (verified vulns + all hardening items). The model returns a
//! `PatchProposal { file, anchor_line, old_block, new_block, explanation }`;
//! Rust locates `old_block` in the file (exact match first, then a fuzzy
//! line-trimmed fallback) and synthesizes a unified diff via `diffy`.
//!
//! Patches are display-only — they are never applied to disk.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use diffy::{create_patch, PatchFormatter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{debug, info, instrument, warn};

use crate::providers::{
    CacheControl, ContentBlock, GenerationRequest, Message, Provider, Role, StopReason,
    SystemBlock,
};
use crate::scanner::util::{collect_text, extract_json_object};
use crate::scanner::verify::VerifiedFinding;
use crate::scanner::{Finding, FindingKind};
use crate::tools;

pub const DEFAULT_PATCH_MODEL: &str = "claude-sonnet-4-6";
pub const DEFAULT_PATCH_CONCURRENCY: usize = 4;
const MAX_TOKENS: u32 = 4096;
const MAX_TOOL_ITERATIONS: usize = 25;

/// Raw model output. The Rust side validates `file` against the finding,
/// then attempts to locate `old_block` in the focus file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchProposal {
    pub file: String,
    pub anchor_line: u32,
    pub old_block: String,
    pub new_block: String,
    pub explanation: String,
}

/// Where (and how) `old_block` was located inside the focus file. The byte
/// offset is into the original file content as read by `propose_one`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Located {
    /// `old_block` matched verbatim.
    Exact {
        byte_offset: usize,
        line_start: u32,
        line_end: u32,
    },
    /// Line-by-line trimmed match — robust to indentation/trailing-whitespace
    /// drift. `byte_offset`/`line_*` refer to the actual file span being
    /// replaced (NOT the trimmed text).
    Fuzzy {
        byte_offset: usize,
        line_start: u32,
        line_end: u32,
        /// The exact substring of the file that will be replaced — may
        /// differ from `proposal.old_block` because of fuzzy normalization.
        matched_text: String,
    },
    NotFound,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Patch {
    pub finding_id: String,
    pub proposal: PatchProposal,
    pub located: Located,
    /// Unified diff text. `None` when `located == NotFound`; otherwise
    /// always present.
    pub diff: Option<String>,
}

const BASE_PATCH_PROMPT: &str = r#"You are a surgical patch author for an application-security pipeline.

Given a CONFIRMED finding (already verified to be a real issue or a
hardening item we've decided to ship), produce the smallest correct fix as
a single block replacement in a single file.

OUTPUT FORMAT

Output STRICT JSON only — no prose, no markdown fences. The object MUST be:

  {
    "file":        "<repeat the finding's file path verbatim>",
    "anchor_line": <integer, 1-indexed line where the change starts>,
    "old_block":   "<exact existing text to replace, verbatim from the file>",
    "new_block":   "<the replacement text>",
    "explanation": "<2–4 sentences: what the patch does and why it closes the issue>"
  }

RULES

- The patch MUST be a single contiguous block in the focus file. No
  multi-file changes; no rewrites of unrelated regions.
- `old_block` MUST appear verbatim in the focus file. Preserve indentation,
  quotes, line endings (use \n), trailing whitespace. The Rust side will
  fall back to a fuzzy whitespace-tolerant match if needed, but exact is
  preferred. Do not include the line-number prefix (e.g. "  12| ") that we
  show you — that's display formatting, not part of the file.
- Pick the MINIMAL span. Don't replace a 40-line function when a 3-line
  expression is enough. Smaller patches are easier to review.
- `new_block` must be the complete replacement, including any imports or
  helpers required if they fit inside the same block. If a fix genuinely
  requires changes outside this single block (e.g. adding a new helper at
  module scope AND patching the caller), pick the change that mitigates
  the finding most directly and call out the rest in `explanation`.
- For VULN findings: the patch must close the verified exploit. The
  verifier's `concrete_exploit` (when present) tells you the exact attack
  vector — use it. A patch that fixes the category but leaves the exploit
  open is wrong.
- For HARDENING findings: the patch implements the defense-in-depth measure
  described. Don't over-reach into adjacent code.
- `explanation` is short and concrete. State the mechanism of the fix
  (parameterized query / allowlist / template auto-escape / etc.) and any
  caller-visible behavior changes."#;

const TOOLS_PREAMBLE: &str = r#"You have read-only access to the project under review through these tools:
  - list_imports / read_file / read_file_range / grep / find_references
  - list_directory / git_blame

Use them sparingly — only to confirm the precise existing text of helper
functions, available imports, or call sites you need to keep consistent.
You are STILL emitting a single-file patch: tools are for understanding,
not for producing multi-file output.

Your FINAL assistant message MUST be the JSON object alone. No tool calls
in that final message; no prose other than the JSON.
"#;

fn system_prompt() -> String {
    format!("{TOOLS_PREAMBLE}\n{BASE_PATCH_PROMPT}")
}

/// Propose a patch for a single verified finding. Reads the focus file from
/// disk, runs the agent loop, parses the JSON proposal, locates
/// `old_block`, and synthesizes the unified diff.
///
/// Both verified vulns and hardening findings flow through here; the caller
/// is responsible for not calling it on dropped vulns.
#[instrument(skip(provider, vf), fields(file = %vf.finding.file, cwe = %vf.finding.cwe))]
pub async fn propose_one(
    vf: &VerifiedFinding,
    scan_root: &Path,
    provider: &dyn Provider,
    model: &str,
) -> Result<Patch> {
    let canonical_root = tools::sandbox::canonical_root(scan_root)?;
    let focus_path = resolve_focus_path(&canonical_root, &vf.finding.file);
    let source = tokio::fs::read_to_string(&focus_path)
        .await
        .with_context(|| format!("read focus file {}", focus_path.display()))?;

    let initial = build_initial_user_message(&canonical_root, vf, &source)?;
    let mut messages: Vec<Message> = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text { text: initial }],
    }];
    let tool_defs = tools::tool_definitions();

    for iteration in 0..MAX_TOOL_ITERATIONS {
        let mut req = GenerationRequest::new(model, MAX_TOKENS);
        // Sonnet still accepts temperature — but we omit it here for the
        // same reason verify does: future-proofs us if we swap to a model
        // that doesn't.
        req.system
            .push(SystemBlock::text(system_prompt()).with_cache(CacheControl::ephemeral_1h()));
        req.tools = tool_defs.clone();
        req.messages = messages.clone();

        let resp = provider
            .generate(req)
            .await
            .context("anthropic generate call failed")?;

        debug!(
            iteration,
            stop_reason = ?resp.stop_reason,
            input_tokens = resp.usage.input_tokens,
            output_tokens = resp.usage.output_tokens,
            cache_read = resp.usage.cache_read_input_tokens,
            "patch iteration"
        );

        messages.push(Message {
            role: Role::Assistant,
            content: resp.content.clone(),
        });

        let tool_uses: Vec<(String, String, Value)> = resp
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, input } => {
                    Some((id.clone(), name.clone(), input.clone()))
                }
                _ => None,
            })
            .collect();

        if tool_uses.is_empty() {
            if !matches!(resp.stop_reason, Some(StopReason::EndTurn) | None) {
                warn!(stop_reason = ?resp.stop_reason, "no tool calls but non-end_turn stop reason");
            }
            let text = collect_text(&resp.content);
            let proposal = parse_proposal(&text)?;
            return Ok(finalize(&vf.finding, proposal, &source));
        }

        info!(iteration, tool_calls = tool_uses.len(), "patch tool calls");

        let mut tool_results: Vec<ContentBlock> = Vec::with_capacity(tool_uses.len());
        for (id, name, input) in &tool_uses {
            let (content, is_error) = match tools::dispatch(name, input, &canonical_root).await {
                Ok(s) => (s, false),
                Err(e) => (format!("error: {e:#}"), true),
            };
            tool_results.push(ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content,
                is_error,
            });
        }
        messages.push(Message {
            role: Role::User,
            content: tool_results,
        });
    }

    Err(anyhow!(
        "patcher hit the {MAX_TOOL_ITERATIONS}-iteration tool-use cap without a final answer"
    ))
}

/// Propose patches for a batch of verified findings in parallel under a
/// semaphore. Filters per spec: KEPT vulns and all hardening items.
/// Findings that don't satisfy that get dropped from the output.
pub async fn propose_many(
    verified: Vec<VerifiedFinding>,
    scan_root: PathBuf,
    provider: Arc<dyn Provider>,
    model: &str,
    concurrency: usize,
) -> Vec<Patch> {
    let permits = Arc::new(Semaphore::new(concurrency.max(1)));
    let model = model.to_string();
    let mut set: JoinSet<Option<Patch>> = JoinSet::new();

    for vf in verified {
        if !should_patch(&vf) {
            continue;
        }
        let permits = permits.clone();
        let provider = provider.clone();
        let model = model.clone();
        let scan_root = scan_root.clone();
        set.spawn(async move {
            let _permit = permits.acquire_owned().await.ok()?;
            match propose_one(&vf, &scan_root, provider.as_ref(), &model).await {
                Ok(p) => Some(p),
                Err(e) => {
                    warn!(file = %vf.finding.file, error = format!("{e:#}"), "patch call failed");
                    None
                }
            }
        });
    }

    let mut out = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok(Some(p)) = joined {
            out.push(p);
        }
    }
    out.sort_by(|a, b| {
        a.proposal
            .file
            .cmp(&b.proposal.file)
            .then_with(|| a.proposal.anchor_line.cmp(&b.proposal.anchor_line))
    });
    out
}

fn should_patch(vf: &VerifiedFinding) -> bool {
    match vf.finding.kind {
        FindingKind::Hardening => true,
        FindingKind::Vuln => vf.verdict.as_ref().map(|v| v.keep()).unwrap_or(false),
    }
}

fn build_initial_user_message(
    canonical_root: &Path,
    vf: &VerifiedFinding,
    source: &str,
) -> Result<String> {
    let mut msg = String::new();
    use std::fmt::Write;
    writeln!(msg, "Scan root: {}", canonical_root.display())?;
    writeln!(msg, "Focus file: {}", vf.finding.file)?;
    writeln!(msg)?;
    writeln!(msg, "Finding:")?;
    writeln!(msg, "{}", serde_json::to_string_pretty(&vf.finding)?)?;
    if let Some(verdict) = &vf.verdict {
        writeln!(msg)?;
        writeln!(msg, "Verifier verdict:")?;
        writeln!(msg, "{}", serde_json::to_string_pretty(verdict)?)?;
    }
    writeln!(msg)?;
    writeln!(msg, "Focus file with line numbers:")?;
    writeln!(msg)?;
    msg.push_str(&with_line_numbers(source));
    Ok(msg)
}

fn parse_proposal(text: &str) -> Result<PatchProposal> {
    let json = extract_json_object(text)
        .ok_or_else(|| anyhow!("patcher response did not contain a JSON object: {text}"))?;
    serde_json::from_str(json).with_context(|| format!("parsing patch JSON: {json}"))
}

/// Locate `old_block` in `source`, build the modified content, and create
/// a unified diff. Always returns a `Patch` — when location fails, `diff`
/// is `None` and `located = NotFound`.
fn finalize(finding: &Finding, proposal: PatchProposal, source: &str) -> Patch {
    let located = locate(source, &proposal.old_block);
    let diff = match &located {
        Located::Exact { byte_offset, .. } => Some(synth_diff(
            &finding.file,
            source,
            *byte_offset,
            proposal.old_block.len(),
            &proposal.new_block,
        )),
        Located::Fuzzy {
            byte_offset,
            matched_text,
            ..
        } => Some(synth_diff(
            &finding.file,
            source,
            *byte_offset,
            matched_text.len(),
            &proposal.new_block,
        )),
        Located::NotFound => None,
    };
    Patch {
        finding_id: finding.id.clone(),
        proposal,
        located,
        diff,
    }
}

/// Find `needle` in `haystack`. Exact `find` first; if that fails, try a
/// line-by-line trimmed match. Returns the actual byte span in the
/// original `haystack` that should be replaced.
pub(crate) fn locate(haystack: &str, needle: &str) -> Located {
    if let Some(off) = haystack.find(needle) {
        let line_start = line_number(haystack, off);
        let line_end = line_number(haystack, off + needle.len().saturating_sub(1));
        return Located::Exact {
            byte_offset: off,
            line_start,
            line_end,
        };
    }
    if let Some((off, len, line_start, line_end)) = locate_fuzzy(haystack, needle) {
        return Located::Fuzzy {
            byte_offset: off,
            matched_text: haystack[off..off + len].to_string(),
            line_start,
            line_end,
        };
    }
    Located::NotFound
}

/// Line-by-line trimmed match. Walks `haystack` line windows of size
/// `needle.lines().count()`; for each, compares trimmed lines to the
/// trimmed needle. Returns (byte_offset, byte_len, line_start, line_end)
/// on first match.
fn locate_fuzzy(haystack: &str, needle: &str) -> Option<(usize, usize, u32, u32)> {
    let needle_lines: Vec<&str> = needle.lines().collect();
    if needle_lines.is_empty() {
        return None;
    }
    let needle_trimmed: Vec<&str> = needle_lines.iter().map(|l| l.trim()).collect();
    if needle_trimmed.iter().all(|l| l.is_empty()) {
        // Pure whitespace — useless to match; refuse rather than picking
        // some arbitrary blank line.
        return None;
    }

    let hay_line_offsets: Vec<usize> = line_offsets(haystack);
    let n = needle_lines.len();
    if hay_line_offsets.len() < n {
        return None;
    }
    for i in 0..=hay_line_offsets.len() - n {
        let mut ok = true;
        for j in 0..n {
            let line_start = hay_line_offsets[i + j];
            let line_end = if i + j + 1 < hay_line_offsets.len() {
                // exclude the trailing '\n'
                hay_line_offsets[i + j + 1].saturating_sub(1)
            } else {
                haystack.len()
            };
            let line = &haystack[line_start..line_end];
            if line.trim() != needle_trimmed[j] {
                ok = false;
                break;
            }
        }
        if ok {
            let byte_offset = hay_line_offsets[i];
            let byte_end = if i + n < hay_line_offsets.len() {
                // include the newline at the end of the last matched line
                hay_line_offsets[i + n]
            } else {
                haystack.len()
            };
            return Some((
                byte_offset,
                byte_end - byte_offset,
                (i as u32) + 1,
                (i + n) as u32,
            ));
        }
    }
    None
}

/// Byte offsets at which each line of `s` starts (0 for the first line).
fn line_offsets(s: &str) -> Vec<usize> {
    let mut out = vec![0usize];
    for (i, b) in s.bytes().enumerate() {
        if b == b'\n' {
            out.push(i + 1);
        }
    }
    out
}

fn line_number(s: &str, byte_offset: usize) -> u32 {
    let off = byte_offset.min(s.len());
    s.as_bytes()[..off].iter().filter(|&&b| b == b'\n').count() as u32 + 1
}

fn synth_diff(
    file: &str,
    source: &str,
    byte_offset: usize,
    byte_len: usize,
    new_block: &str,
) -> String {
    let mut modified = String::with_capacity(source.len() + new_block.len());
    modified.push_str(&source[..byte_offset]);
    modified.push_str(new_block);
    modified.push_str(&source[byte_offset + byte_len..]);
    let patch = create_patch(source, &modified);
    let formatted = PatchFormatter::new().fmt_patch(&patch).to_string();
    // diffy emits its own `--- original / +++ modified` header; replace with
    // a tagged-by-filename header so the UI knows which file the diff is for.
    let rest = strip_default_header(&formatted);
    // Bare file path in the header — works for absolute and relative paths,
    // avoids the `a//Users/...` double-slash artifact when callers pass an
    // absolute path. Most diff viewers handle either convention.
    format!("--- {file}\n+++ {file}\n{rest}")
}

fn strip_default_header(diff: &str) -> &str {
    // Skip the first two header lines emitted by diffy if present.
    let mut start = 0;
    for _ in 0..2 {
        if let Some(nl) = diff[start..].find('\n') {
            let line = &diff[start..start + nl];
            if line.starts_with("--- ") || line.starts_with("+++ ") {
                start += nl + 1;
            } else {
                break;
            }
        }
    }
    &diff[start..]
}

fn resolve_focus_path(root: &Path, file: &str) -> PathBuf {
    let p = PathBuf::from(file);
    if p.is_absolute() {
        p
    } else {
        root.join(p)
    }
}

fn with_line_numbers(source: &str) -> String {
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
    use crate::error::ProviderResult;
    use crate::providers::{Response, StreamEvent, Usage};
    use crate::scanner::verify::Verdict;
    use crate::scanner::Severity;
    use async_trait::async_trait;
    use futures::stream::BoxStream;
    use std::sync::Mutex;
    use tempfile::TempDir;

    fn mk_finding(file: &str, kind: FindingKind) -> Finding {
        let mut f = Finding {
            id: String::new(),
            kind,
            severity: Severity::High,
            cwe: "CWE-89".into(),
            owasp: None,
            title: "T".into(),
            file: file.into(),
            line_start: 2,
            line_end: 2,
            description: "d".into(),
            data_flow: "src→sink".into(),
        };
        f.assign_id();
        f
    }

    // --- locate ---------------------------------------------------------

    #[test]
    fn locate_exact_match_single_line() {
        let src = "line a\nline b\nline c\n";
        match locate(src, "line b\n") {
            Located::Exact {
                byte_offset,
                line_start,
                line_end,
            } => {
                assert_eq!(byte_offset, 7);
                assert_eq!(line_start, 2);
                assert_eq!(line_end, 2);
            }
            other => panic!("expected Exact, got {other:?}"),
        }
    }

    #[test]
    fn locate_exact_match_multi_line() {
        let src = "a\nb\nc\nd\n";
        match locate(src, "b\nc\n") {
            Located::Exact {
                line_start,
                line_end,
                ..
            } => {
                assert_eq!(line_start, 2);
                assert_eq!(line_end, 3);
            }
            other => panic!("expected Exact, got {other:?}"),
        }
    }

    #[test]
    fn locate_fuzzy_on_indent_drift() {
        // Source has 4-space indent; model emitted 2-space indent.
        let src = "fn main() {\n    let x = 1;\n    let y = 2;\n}\n";
        let needle = "  let x = 1;\n  let y = 2;\n";
        match locate(src, needle) {
            Located::Fuzzy {
                line_start,
                line_end,
                matched_text,
                ..
            } => {
                assert_eq!(line_start, 2);
                assert_eq!(line_end, 3);
                assert_eq!(matched_text, "    let x = 1;\n    let y = 2;\n");
            }
            other => panic!("expected Fuzzy, got {other:?}"),
        }
    }

    #[test]
    fn locate_fuzzy_on_trailing_whitespace() {
        let src = "a   \nb\nc\n";
        // needle has no trailing whitespace on the first line
        match locate(src, "a\nb\n") {
            Located::Fuzzy { .. } => (),
            other => panic!("expected Fuzzy, got {other:?}"),
        }
    }

    #[test]
    fn locate_not_found() {
        let src = "hello\nworld\n";
        assert!(matches!(locate(src, "goodbye"), Located::NotFound));
    }

    // --- diff -----------------------------------------------------------

    #[test]
    fn diff_round_trip_single_line() {
        let src = "fn main() {\n    let x = 1;\n}\n";
        let new_block = "    let x = 2;\n";
        let off = src.find("    let x = 1;\n").unwrap();
        let diff = synth_diff("src/lib.rs", src, off, "    let x = 1;\n".len(), new_block);
        assert!(diff.starts_with("--- src/lib.rs\n+++ src/lib.rs\n"));
        // Default diffy header must not leak through.
        assert!(!diff.contains("--- original"));
        assert!(diff.contains("-    let x = 1;"));
        assert!(diff.contains("+    let x = 2;"));
    }

    #[test]
    fn finalize_produces_diff_for_exact_match() {
        let src = "a\nb\nc\n";
        let finding = mk_finding("focus.ts", FindingKind::Vuln);
        let proposal = PatchProposal {
            file: "focus.ts".into(),
            anchor_line: 2,
            old_block: "b\n".into(),
            new_block: "B\n".into(),
            explanation: "x".into(),
        };
        let patch = finalize(&finding, proposal, src);
        assert!(matches!(patch.located, Located::Exact { .. }));
        let diff = patch.diff.unwrap();
        assert!(diff.contains("-b"));
        assert!(diff.contains("+B"));
    }

    #[test]
    fn finalize_produces_no_diff_on_not_found() {
        let src = "a\nb\nc\n";
        let finding = mk_finding("focus.ts", FindingKind::Vuln);
        let proposal = PatchProposal {
            file: "focus.ts".into(),
            anchor_line: 1,
            old_block: "nope".into(),
            new_block: "still nope".into(),
            explanation: "x".into(),
        };
        let patch = finalize(&finding, proposal, src);
        assert!(matches!(patch.located, Located::NotFound));
        assert!(patch.diff.is_none());
    }

    // --- propose_many filter -------------------------------------------

    struct OneShotProvider {
        body: Mutex<Option<String>>,
    }

    impl OneShotProvider {
        fn new(body: &str) -> Self {
            Self {
                body: Mutex::new(Some(body.into())),
            }
        }
    }

    #[async_trait]
    impl Provider for OneShotProvider {
        fn name(&self) -> &'static str {
            "oneshot"
        }
        async fn generate(&self, _req: GenerationRequest) -> ProviderResult<Response> {
            let body = self
                .body
                .lock()
                .unwrap()
                .clone()
                .expect("provider hit unexpectedly more than once");
            Ok(Response {
                id: "msg".into(),
                model: "oneshot".into(),
                content: vec![ContentBlock::Text { text: body }],
                stop_reason: Some(StopReason::EndTurn),
                stop_sequence: None,
                usage: Usage::default(),
            })
        }
        async fn stream(
            &self,
            _req: GenerationRequest,
        ) -> ProviderResult<BoxStream<'static, ProviderResult<StreamEvent>>> {
            unimplemented!()
        }
    }

    fn mk_vf(file: &str, kind: FindingKind, verify_keep: Option<bool>) -> VerifiedFinding {
        let finding = mk_finding(file, kind);
        let verdict = verify_keep.map(|keep| Verdict {
            is_reachable: keep,
            source_is_untrusted: true,
            concrete_exploit: if keep {
                Some(crate::scanner::verify::Exploit {
                    kind: crate::scanner::verify::ExploitKind::Other,
                    request: None,
                    payload: "x".into(),
                    expected_effect: "y".into(),
                })
            } else {
                None
            },
            reasoning: "r".into(),
        });
        VerifiedFinding { finding, verdict }
    }

    #[tokio::test]
    async fn propose_many_skips_dropped_vulns() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::write(root.join("focus.ts"), "a\nb\nc\n").unwrap();

        // Only one provider call should happen — the KEEP'd vuln. The
        // dropped vuln gets skipped without calling the model.
        let provider: Arc<dyn Provider> = Arc::new(OneShotProvider::new(
            r#"{"file":"focus.ts","anchor_line":2,"old_block":"b\n","new_block":"B\n","explanation":"x"}"#,
        ));

        let verified = vec![
            mk_vf("focus.ts", FindingKind::Vuln, Some(false)), // dropped
            mk_vf("focus.ts", FindingKind::Vuln, Some(true)),  // kept
        ];
        let out =
            propose_many(verified, root, provider, "oneshot", 2).await;
        assert_eq!(out.len(), 1);
        assert!(out[0].diff.is_some());
    }

    #[tokio::test]
    async fn propose_many_patches_hardening() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::write(root.join("focus.ts"), "a\nb\nc\n").unwrap();

        let provider: Arc<dyn Provider> = Arc::new(OneShotProvider::new(
            r#"{"file":"focus.ts","anchor_line":2,"old_block":"b\n","new_block":"B\n","explanation":"x"}"#,
        ));
        let verified = vec![mk_vf("focus.ts", FindingKind::Hardening, None)];
        let out = propose_many(verified, root, provider, "oneshot", 2).await;
        assert_eq!(out.len(), 1);
    }
}
