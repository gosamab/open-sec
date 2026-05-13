use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use tracing::{debug, instrument};

use crate::providers::{
    CacheControl, ContentBlock, GenerationRequest, Message, Provider, Role, SystemBlock,
};
use crate::scanner::Finding;

pub const DEFAULT_DETECT_MODEL: &str = "claude-sonnet-4-6";
const DEFAULT_MAX_TOKENS: u32 = 8192;

const DETECTION_SYSTEM_PROMPT: &str = r#"You are a meticulous application-security reviewer.

Given a single source file, identify two classes of issues:
1. "vuln"      — concrete vulnerabilities with a describable source → sink path.
2. "hardening" — defense-in-depth opportunities that do not claim an exploitable bug.

Output STRICT JSON only — no prose, no markdown fences, no commentary.
The top-level object must be: {"findings": [ ... ]}.

Each finding MUST contain every field below:
  kind:        "vuln" | "hardening"
  severity:    "critical" | "high" | "medium" | "low" | "info"
  cwe:         "CWE-<number>"        (required; pick the best-fit CWE)
  owasp:       "A<NN>:<year>" | null (optional OWASP Top 10 mapping)
  title:       <one-line summary, <= 80 chars>
  file:        <repeat the file path from the user message verbatim>
  line_start:  <integer, 1-indexed>
  line_end:    <integer, 1-indexed, inclusive>
  description: <2–4 sentences explaining the issue>
  data_flow:   <source → transformation → sink narrative, even for "hardening">

SEVERITY GUIDE (apply strictly to avoid noise):
  critical — unauthenticated remote RCE, authentication bypass, exposed secrets.
  high     — SQLi / SSRF / SSTI / path traversal exploitable in normal use;
             auth-required RCE; deserialization of attacker input.
  medium   — requires a specific non-default state or limited privileges.
  low      — defense-in-depth gaps, minor information disclosure.
  info     — stylistic / best-practice only. NEVER use for actual vulnerabilities.

RULES:
- Every "vuln" MUST describe a concrete source → sink path.
- Generic advice ("consider parameterized queries") is "hardening", not "vuln".
- Do NOT emit a "hardening" item whose substance is the mitigation, restatement,
  or sub-aspect of a "vuln" you already reported in this same response. Examples:
  if you reported SQL injection, do NOT also emit "missing input validation" or
  "use parameterized queries" as hardening. If you reported command injection,
  do NOT also emit "use argv form instead of shell=True" or "validate host" as
  hardening. Hardening items must describe a DISTINCT issue with its own
  source → sink narrative — not implied by the vuln's existence.
- line_start / line_end must reference real lines in the file you were given.
  Prefer the minimal span that contains the source → sink, not the whole
  enclosing function.
- If you find nothing, return {"findings": []}.
- Do NOT add fields not listed above. Do NOT wrap the JSON in markdown."#;

#[derive(Deserialize)]
struct FindingsEnvelope {
    findings: Vec<Finding>,
}

#[instrument(skip(provider, source), fields(path = %path.display(), bytes = source.len()))]
pub async fn scan_single_file(
    path: &Path,
    source: &str,
    provider: &dyn Provider,
    model: &str,
) -> Result<Vec<Finding>> {
    let file_label = path.display().to_string();

    let mut req = GenerationRequest::new(model, DEFAULT_MAX_TOKENS);
    req.temperature = Some(0.0);
    req.system.push(
        SystemBlock::text(DETECTION_SYSTEM_PROMPT).with_cache(CacheControl::ephemeral_1h()),
    );
    req.messages.push(Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: format!("File: {file_label}\n\n{}", with_line_numbers(source)),
        }],
    });

    let resp = provider
        .generate(req)
        .await
        .context("anthropic generate call failed")?;

    let text = collect_text(&resp.content);
    debug!(chars = text.len(), "received detection response");

    let json = extract_json_object(&text)
        .ok_or_else(|| anyhow!("model response did not contain a JSON object: {text}"))?;

    let envelope: FindingsEnvelope =
        serde_json::from_str(json).with_context(|| format!("parsing findings JSON: {json}"))?;

    let mut findings = envelope.findings;
    for f in &mut findings {
        // The model is asked to echo the file path, but normalize to the caller's
        // path so the stable id is computed from a single source of truth.
        f.file = file_label.clone();
        f.assign_id();
    }
    Ok(findings)
}

fn collect_text(content: &[ContentBlock]) -> String {
    let mut out = String::new();
    for block in content {
        if let ContentBlock::Text { text } = block {
            out.push_str(text);
        }
    }
    out
}

/// Extract a top-level JSON object from arbitrary text. Strips ```json fences
/// the model occasionally adds despite being told not to.
fn extract_json_object(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    let stripped = strip_fence(trimmed).unwrap_or(trimmed);
    let start = stripped.find('{')?;
    let end = matching_close(&stripped[start..])? + start;
    Some(&stripped[start..=end])
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
    fn line_numbers_pad_to_width() {
        let out = with_line_numbers("a\nb\nc");
        // 3 lines → width 3
        assert!(out.starts_with("  1| a\n"));
        assert!(out.contains("  2| b\n"));
        assert!(out.contains("  3| c"));
    }
}
