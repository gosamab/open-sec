//! Verification pass — Opus adversarially re-examines each `vuln` finding
//! and decides whether the source → sink path is actually reachable. Outputs
//! a structured `Verdict`; a finding is kept iff `is_reachable` and a
//! `concrete_exploit` was constructed.
//!
//! Hardening findings bypass the verifier entirely (per CLAUDE.md): they
//! describe defense-in-depth gaps, not reachable bugs, so adversarial
//! reachability analysis doesn't apply.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
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
use crate::scanner::{Finding, FindingKind};
use crate::tools;

pub const DEFAULT_VERIFY_MODEL: &str = "claude-opus-4-7";
pub const DEFAULT_VERIFY_CONCURRENCY: usize = 2;
const MAX_TOKENS: u32 = 4096;
const MAX_TOOL_ITERATIONS: usize = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExploitKind {
    /// Exploit driven through an HTTP request. Use `request` to describe it.
    Http,
    /// Exploit driven through function/CLI/process arguments. Describe in `payload`.
    Args,
    /// Exploit driven through a crafted file path or file content.
    File,
    /// Anything else (IPC, env var, deserializer, ...). Free-form in `payload`.
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Exploit {
    pub kind: ExploitKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<HttpRequest>,
    /// The malicious input itself (a string, JSON snippet, or descriptive
    /// fragment). For `kind=http`, this is typically the dangerous field
    /// value from the request.
    pub payload: String,
    /// One-line description of what the attacker gains (e.g. "auth bypass",
    /// "arbitrary file read", "remote code execution").
    pub expected_effect: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verdict {
    pub is_reachable: bool,
    pub source_is_untrusted: bool,
    /// Present iff the verifier could construct a concrete exploit. Required
    /// for the finding to be kept.
    #[serde(default)]
    pub concrete_exploit: Option<Exploit>,
    pub reasoning: String,
}

impl Verdict {
    /// Decision rule per CLAUDE.md: keep iff reachable AND a concrete exploit
    /// was produced.
    pub fn keep(&self) -> bool {
        self.is_reachable && self.concrete_exploit.is_some()
    }
}

/// A finding plus its verifier verdict. The orchestrator decides what to do
/// with the pair based on `verdict.keep()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifiedFinding {
    pub finding: Finding,
    /// `None` for hardening findings (they bypass the verifier) and for
    /// findings where the verifier call failed.
    pub verdict: Option<Verdict>,
}

const BASE_VERIFY_PROMPT: &str = r#"You are an adversarial verifier for an application-security pipeline.

A prior detection pass has flagged a candidate vulnerability. Your job is to
prove or disprove it the way a skeptical reviewer would: trace the actual
source → sink path, confirm the source carries untrusted input under normal
operation, and construct a concrete exploit. If you cannot, the finding is
discarded.

You are NOT looking for new bugs. Focus only on the supplied finding.

OUTPUT FORMAT

Output STRICT JSON only — no prose, no markdown fences. Shape:

  {
    "is_reachable": <bool>,
    "source_is_untrusted": <bool>,
    "concrete_exploit": null | {
      "kind": "http" | "args" | "file" | "other",
      "request": null | { "method": "...", "path": "...",
                          "headers": null | { ... },
                          "body": null | <any JSON> },
      "payload": "<the malicious input itself>",
      "expected_effect": "<one-line attacker gain, e.g. 'auth bypass'>"
    },
    "reasoning": "<2–5 sentences justifying the verdict>"
  }

FIELD RULES:
  - `is_reachable` — true iff a normal call path from an external caller
    arrives at the sink with attacker-controlled data.
  - `source_is_untrusted` — true iff the source identified in the finding's
    data_flow carries data the attacker can influence in production (HTTP
    body/query/header, file path arg, env var the attacker controls, etc.).
  - `concrete_exploit` — REQUIRED when `is_reachable` is true. null otherwise.
    Use `kind: "http"` and fill `request` when the exploit goes through a
    network endpoint. Use `kind: "args"`/"file"/"other" for non-HTTP entry
    points; `request` is null then. `payload` is always present and is the
    malicious input itself, not a description of it.
  - `reasoning` — adversarial. State WHY this is or isn't real. If discarding,
    say what assumption the detection pass made that doesn't hold.

DECISION GUIDE:
  - Sanitization that the detection pass missed: not reachable.
  - Source that requires admin auth or a trusted internal caller: source not
    untrusted. Still mark `is_reachable` true if the call path exists, but
    omit the exploit and the finding will be discarded.
  - Detection cited a sink in a dead branch / unexported function with no
    callers: not reachable.
  - When unsure whether a helper is sanitizing or pass-through, READ IT with
    the tools before deciding."#;

const TOOLS_PREAMBLE: &str = r#"You have read-only access to the project under review through these tools:
  - list_imports / read_file / read_file_range / grep / find_references
  - list_directory / git_blame

Use them adversarially: chase the source → sink path further than the
detection pass did. Read sanitizer helpers, check for callers/gates around
the sink, and confirm the source really is reachable in production. Each
tool call costs tokens — be parsimonious; stop as soon as you have what you
need.

Your FINAL assistant message MUST be the JSON object alone. No tool calls
in that final message; no prose other than the JSON.
"#;

fn system_prompt() -> String {
    format!("{TOOLS_PREAMBLE}\n{BASE_VERIFY_PROMPT}")
}

/// Verify a single finding against its focus file. Hardening findings bypass
/// the verifier (returns `VerifiedFinding { verdict: None }`). For `vuln`
/// findings, runs the agent loop and returns the parsed verdict.
#[instrument(skip(provider, finding), fields(file = %finding.file, cwe = %finding.cwe))]
pub async fn verify_one(
    finding: Finding,
    scan_root: &Path,
    provider: &dyn Provider,
    model: &str,
) -> Result<VerifiedFinding> {
    if matches!(finding.kind, FindingKind::Hardening) {
        return Ok(VerifiedFinding {
            finding,
            verdict: None,
        });
    }

    let canonical_root = tools::sandbox::canonical_root(scan_root)?;
    let focus_path = resolve_focus_path(&canonical_root, &finding.file);
    let source = tokio::fs::read_to_string(&focus_path)
        .await
        .with_context(|| format!("read focus file {}", focus_path.display()))?;

    let initial = format!(
        "Scan root: {root}\nFocus file: {file}\n\nCandidate finding (from detection):\n{finding}\n\nFocus file with line numbers:\n\n{src}",
        root = canonical_root.display(),
        file = finding.file,
        finding = serde_json::to_string_pretty(&finding)?,
        src = with_line_numbers(&source),
    );

    let mut messages: Vec<Message> = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text { text: initial }],
    }];
    let tool_defs = tools::tool_definitions();

    for iteration in 0..MAX_TOOL_ITERATIONS {
        // No `temperature` — Opus 4.7 rejects it ("deprecated for this model").
        // Sonnet/Haiku silently accept its absence, so leaving it unset is
        // the most-compatible choice across the three stage models.
        let mut req = GenerationRequest::new(model, MAX_TOKENS);
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
            "verify iteration"
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
            let verdict = parse_verdict(&text)?;
            return Ok(VerifiedFinding {
                finding,
                verdict: Some(verdict),
            });
        }

        info!(iteration, tool_calls = tool_uses.len(), "verify tool calls");

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
        "verifier hit the {MAX_TOOL_ITERATIONS}-iteration tool-use cap without a final answer"
    ))
}

/// Verify many findings in parallel. Caps concurrency at `concurrency` (use
/// `DEFAULT_VERIFY_CONCURRENCY` for the CLAUDE.md default). Hardening
/// findings skip the LLM and pass through with `verdict: None`. Failures on
/// individual findings are logged and surface as `verdict: None`.
pub async fn verify_many(
    findings: Vec<Finding>,
    scan_root: PathBuf,
    provider: Arc<dyn Provider>,
    model: &str,
    concurrency: usize,
) -> Vec<VerifiedFinding> {
    let permits = Arc::new(Semaphore::new(concurrency.max(1)));
    let model = model.to_string();
    let mut set: JoinSet<VerifiedFinding> = JoinSet::new();

    for finding in findings {
        let permits = permits.clone();
        let provider = provider.clone();
        let model = model.clone();
        let scan_root = scan_root.clone();
        set.spawn(async move {
            // Hardening shortcut — doesn't touch the semaphore, no API call.
            if matches!(finding.kind, FindingKind::Hardening) {
                return VerifiedFinding {
                    finding,
                    verdict: None,
                };
            }
            let _permit = match permits.acquire_owned().await {
                Ok(p) => p,
                Err(_) => {
                    return VerifiedFinding {
                        finding,
                        verdict: None,
                    }
                }
            };
            match verify_one(finding.clone(), &scan_root, provider.as_ref(), &model).await {
                Ok(v) => v,
                Err(e) => {
                    warn!(file = %finding.file, error = format!("{e:#}"), "verify call failed; passing finding through unverified");
                    VerifiedFinding {
                        finding,
                        verdict: None,
                    }
                }
            }
        });
    }

    let mut out = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok(v) = joined {
            out.push(v);
        }
    }
    // Stable order: by file then line_start.
    out.sort_by(|a, b| {
        a.finding
            .file
            .cmp(&b.finding.file)
            .then_with(|| a.finding.line_start.cmp(&b.finding.line_start))
    });
    out
}

fn parse_verdict(text: &str) -> Result<Verdict> {
    let json = extract_json_object(text)
        .ok_or_else(|| anyhow!("verifier response did not contain a JSON object: {text}"))?;
    serde_json::from_str(json).with_context(|| format!("parsing verdict JSON: {json}"))
}

/// If `finding.file` is absolute and inside `root`, use it directly. Otherwise
/// treat it as a path relative to `root`.
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
    use crate::scanner::Severity;
    use async_trait::async_trait;
    use futures::stream::BoxStream;
    use std::sync::Mutex;
    use tempfile::TempDir;

    struct ScriptedProvider {
        responses: Mutex<Vec<Response>>,
    }

    impl ScriptedProvider {
        fn new(responses: Vec<Response>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl Provider for ScriptedProvider {
        fn name(&self) -> &'static str {
            "scripted"
        }
        async fn generate(&self, _req: GenerationRequest) -> ProviderResult<Response> {
            let mut r = self.responses.lock().unwrap();
            if r.is_empty() {
                panic!("ScriptedProvider out of responses");
            }
            Ok(r.remove(0))
        }
        async fn stream(
            &self,
            _req: GenerationRequest,
        ) -> ProviderResult<BoxStream<'static, ProviderResult<StreamEvent>>> {
            unimplemented!()
        }
    }

    fn text_response(json_body: &str) -> Response {
        Response {
            id: "msg_final".into(),
            model: "scripted".into(),
            content: vec![ContentBlock::Text {
                text: json_body.into(),
            }],
            stop_reason: Some(StopReason::EndTurn),
            stop_sequence: None,
            usage: Usage::default(),
        }
    }

    fn mk_finding(file: &str, kind: FindingKind) -> Finding {
        let mut f = Finding {
            id: String::new(),
            kind,
            severity: Severity::High,
            cwe: "CWE-89".into(),
            owasp: None,
            title: "Test".into(),
            file: file.into(),
            line_start: 1,
            line_end: 2,
            description: "d".into(),
            data_flow: "src→sink".into(),
        };
        f.assign_id();
        f
    }

    #[tokio::test]
    async fn verify_one_parses_kept_verdict() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::write(root.join("focus.ts"), "// vuln\nconst x = 1;\n").unwrap();

        let provider = ScriptedProvider::new(vec![text_response(
            r#"{
              "is_reachable": true,
              "source_is_untrusted": true,
              "concrete_exploit": {
                "kind": "http",
                "request": {"method":"POST","path":"/login","headers":null,
                            "body":{"email":"a@b","password":"' OR 1=1--"}},
                "payload": "' OR 1=1--",
                "expected_effect": "auth bypass"
              },
              "reasoning": "reachable from /login; password concatenated into SQL"
            }"#,
        )]);

        let finding = mk_finding("focus.ts", FindingKind::Vuln);
        let v = verify_one(finding, &root, &provider, "scripted").await.unwrap();
        let verdict = v.verdict.expect("verdict present");
        assert!(verdict.keep());
        let ex = verdict.concrete_exploit.unwrap();
        assert_eq!(ex.kind, ExploitKind::Http);
        assert_eq!(ex.request.unwrap().path, "/login");
        assert_eq!(ex.payload, "' OR 1=1--");
    }

    #[tokio::test]
    async fn verify_one_parses_discarded_verdict() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::write(root.join("focus.ts"), "// safe\nconst x = 1;\n").unwrap();

        let provider = ScriptedProvider::new(vec![text_response(
            r#"{"is_reachable": false, "source_is_untrusted": true,
                "concrete_exploit": null,
                "reasoning": "the sink is gated by a prior check the detection pass missed"}"#,
        )]);
        let finding = mk_finding("focus.ts", FindingKind::Vuln);
        let v = verify_one(finding, &root, &provider, "scripted").await.unwrap();
        let verdict = v.verdict.expect("verdict present");
        assert!(!verdict.keep());
        assert!(verdict.concrete_exploit.is_none());
    }

    #[tokio::test]
    async fn verify_one_skips_hardening() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        // No script — if the verifier tried to call the model, it would panic.
        let provider = ScriptedProvider::new(vec![]);
        let finding = mk_finding("focus.ts", FindingKind::Hardening);
        let v = verify_one(finding, &root, &provider, "scripted").await.unwrap();
        assert!(v.verdict.is_none());
    }

    #[tokio::test]
    async fn verify_many_passes_hardening_through_without_api_calls() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::write(root.join("focus.ts"), "x\n").unwrap();

        // Only one response — for the single vuln. Hardening must not consume one.
        let provider: Arc<dyn Provider> = Arc::new(ScriptedProvider::new(vec![text_response(
            r#"{"is_reachable":true,"source_is_untrusted":true,
                "concrete_exploit":{"kind":"other","payload":"x","expected_effect":"y"},
                "reasoning":"ok"}"#,
        )]));

        let findings = vec![
            mk_finding("focus.ts", FindingKind::Hardening),
            mk_finding("focus.ts", FindingKind::Vuln),
            mk_finding("focus.ts", FindingKind::Hardening),
        ];
        let out = verify_many(findings, root.clone(), provider, "scripted", 2).await;
        assert_eq!(out.len(), 3);
        assert_eq!(out.iter().filter(|v| v.verdict.is_some()).count(), 1);
        assert_eq!(out.iter().filter(|v| v.verdict.is_none()).count(), 2);
    }
}
