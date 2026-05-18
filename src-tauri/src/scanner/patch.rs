//! Patch proposal pass — Sonnet drafts a minimal fix per surviving finding
//! (kept vulns + all hardening items). The model returns a
//! `PatchProposal { file, anchor_line, old_block, new_block, explanation }`;
//! we locate `old_block` (exact, then fuzzy line-trimmed) and synthesize a
//! unified diff via `diffy`. Writing back to disk is gated behind the
//! `apply_patch` IPC, not done here.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use diffy::{create_patch, PatchFormatter};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{instrument, warn};

use crate::providers::Provider;
use crate::scanner::agent_loop::{run_agent_loop, AgentRequest};
use crate::scanner::util::{extract_json_object, resolve_focus_path, with_line_numbers};
use crate::scanner::verify::VerifiedFinding;
use crate::scanner::{Finding, FindingKind};
use crate::tools;

pub const DEFAULT_PATCH_MODEL: &str = "claude-sonnet-4-6";
pub const DEFAULT_PATCH_CONCURRENCY: usize = 4;
const MAX_TOKENS: u32 = 4096;

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


/// Propose a patch for a single verified finding. Reads the focus file from
/// disk, runs the agent loop, parses the JSON proposal, locates
/// `old_block`, and synthesizes the unified diff.
///
/// Both verified vulns and hardening findings flow through here; the caller
/// is responsible for not calling it on dropped vulns.
///
/// `prior_attempts` carries earlier proposals the user has already seen for
/// the same finding — when non-empty, the prompt asks the model to pick a
/// structurally different fix from the listed alternatives.
#[instrument(skip(provider, vf, prior_attempts), fields(file = %vf.finding.file, cwe = %vf.finding.cwe))]
pub async fn propose_one(
    vf: &VerifiedFinding,
    scan_root: &Path,
    provider: &dyn Provider,
    model: &str,
    prior_attempts: &[PatchProposal],
) -> Result<Patch> {
    let canonical_root = tools::sandbox::canonical_root(scan_root)?;
    let focus_path = resolve_focus_path(&canonical_root, &vf.finding.file);
    let source = tokio::fs::read_to_string(&focus_path)
        .await
        .with_context(|| format!("read focus file {}", focus_path.display()))?;

    let initial_user_msg = build_initial_user_message(&canonical_root, vf, &source, prior_attempts)?;

    // Sonnet still accepts temperature — but we omit it here so swapping to
    // a stricter model (the way verify uses Opus) won't break this stage.
    let final_text = run_agent_loop(AgentRequest {
        system_prompt: format!("{TOOLS_PREAMBLE}\n{BASE_PATCH_PROMPT}"),
        initial_user_msg,
        model,
        max_tokens: MAX_TOKENS,
        temperature: None,
        canonical_root: &canonical_root,
        provider,
        stage_label: "patcher",
    })
    .await?;

    let proposal = parse_proposal(&final_text)?;
    Ok(finalize(&vf.finding, proposal, &source))
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
            match propose_one(&vf, &scan_root, provider.as_ref(), &model, &[]).await {
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
    prior_attempts: &[PatchProposal],
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
    if !prior_attempts.is_empty() {
        writeln!(msg)?;
        writeln!(
            msg,
            "PRIOR ATTEMPTS — the user has already seen the following {} proposal(s) and asked for an alternative.",
            prior_attempts.len()
        )?;
        writeln!(
            msg,
            "Propose a STRUCTURALLY DIFFERENT fix: pick a different control point (e.g. validate at the boundary instead of escaping at the sink), a different defense mechanism (allowlist vs sanitizer vs typing), or a different scope. Do NOT just reword the same approach with different identifiers. If the previous attempts were all incorrect, address that directly."
        )?;
        for (i, p) in prior_attempts.iter().enumerate() {
            writeln!(msg)?;
            writeln!(msg, "--- Attempt #{} ---", i + 1)?;
            writeln!(msg, "Explanation: {}", p.explanation)?;
            writeln!(msg, "new_block (replacement):")?;
            writeln!(msg, "{}", p.new_block)?;
        }
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

#[cfg(test)]
mod tests;
