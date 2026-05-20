//! Verification pass — Opus adversarially re-examines each `vuln` finding
//! and emits a `Verdict`. Kept iff `is_reachable && concrete_exploit.is_some()`.
//! Hardening findings skip this stage entirely (defense-in-depth, not bugs).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{instrument, warn};

use crate::providers::{Provider, Tool};
use crate::scanner::agent_loop::{run_agent_loop, AgentRequest};
use crate::scanner::util::{resolve_focus_path, with_line_numbers};
use crate::scanner::{Finding, FindingKind};
use crate::tools;

pub const DEFAULT_VERIFY_MODEL: &str = "claude-sonnet-4-6";
pub const DEFAULT_VERIFY_CONCURRENCY: usize = 2;
const MAX_TOKENS: u32 = 4096;

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

FIELD RULES (the `submit_verdict` tool enforces the JSON shape; these are
semantics):
  - `is_reachable` — true iff a normal call path from an external caller
    arrives at the sink with attacker-controlled data.
  - `source_is_untrusted` — true iff the source identified in the finding's
    data_flow carries data the attacker can influence in production (HTTP
    body/query/header, file path arg, env var the attacker controls, etc.).
  - `concrete_exploit` — REQUIRED when `is_reachable` is true. Omit (or pass
    null) otherwise. Use `kind: "http"` and fill `request` when the exploit
    goes through a network endpoint. Use `kind: "args"`/"file"/"other` for
    non-HTTP entry points; `request` is null then. `payload` is always
    present and is the malicious input itself, not a description of it.
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

When you have your conclusion, call the `submit_verdict` tool with the
structured verdict. That tool call IS your final answer — do not also
reply in free text afterward.
"#;

fn submit_verdict_tool() -> Tool {
    Tool {
        name: "submit_verdict".to_string(),
        description: "Submit your final verdict for the candidate finding. \
                      Call this exactly once when you have decided whether the \
                      finding is reachable with a concrete exploit. Do not reply \
                      in free text after calling this — the tool call itself is \
                      your final answer."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "is_reachable": {
                    "type": "boolean",
                    "description": "True iff a normal call path from an external caller arrives at the sink with attacker-controlled data."
                },
                "source_is_untrusted": {
                    "type": "boolean",
                    "description": "True iff the source carries data the attacker can influence in production."
                },
                "concrete_exploit": {
                    "type": ["object", "null"],
                    "additionalProperties": false,
                    "description": "Required when is_reachable is true; null otherwise.",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["http", "args", "file", "other"],
                            "description": "Exploit delivery vector."
                        },
                        "request": {
                            "type": ["object", "null"],
                            "additionalProperties": false,
                            "description": "Set when kind is http; null otherwise.",
                            "properties": {
                                "method":  { "type": "string" },
                                "path":    { "type": "string" },
                                "headers": { "type": ["string", "null"], "description": "Headers as text or JSON-encoded string; null if not applicable." },
                                "body":    { "type": ["string", "null"], "description": "Body as text or JSON-encoded string; null if not applicable." }
                            },
                            "required": ["method", "path", "headers", "body"]
                        },
                        "payload": {
                            "type": "string",
                            "description": "The malicious input itself, not a description of it."
                        },
                        "expected_effect": {
                            "type": "string",
                            "description": "One-line attacker gain (e.g. 'auth bypass', 'arbitrary file read', 'RCE')."
                        }
                    },
                    "required": ["kind", "request", "payload", "expected_effect"]
                },
                "reasoning": {
                    "type": "string",
                    "description": "2-5 sentences justifying the verdict. Adversarial."
                }
            },
            "required": ["is_reachable", "source_is_untrusted", "concrete_exploit", "reasoning"]
        }),
        cache_control: None,
    }
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

    let initial_user_msg = format!(
        "Scan root: {root}\nFocus file: {file}\n\nCandidate finding (from detection):\n{finding}\n\nFocus file with line numbers:\n\n{src}",
        root = canonical_root.display(),
        file = finding.file,
        finding = serde_json::to_string_pretty(&finding)?,
        src = with_line_numbers(&source),
    );

    // No `temperature` — Opus 4.7 rejects it ("deprecated for this model");
    // Sonnet/Haiku silently accept its absence, so unset is the most
    // compatible choice across the three stage models.
    let tool_input = run_agent_loop(
        AgentRequest {
            system_prompt: format!("{TOOLS_PREAMBLE}\n{BASE_VERIFY_PROMPT}"),
            initial_user_msg,
            model,
            max_tokens: MAX_TOKENS,
            temperature: None,
            canonical_root: &canonical_root,
            provider,
            stage_label: "verifier",
        },
        submit_verdict_tool(),
    )
    .await?;

    let verdict: Verdict = serde_json::from_value(tool_input)
        .context("submit_verdict input did not match Verdict schema")?;
    Ok(VerifiedFinding {
        finding,
        verdict: Some(verdict),
    })
}

/// Fires once per verifier task as it finishes (success, hardening
/// pass-through, or error). The caller owns the "done out of total" math —
/// this exists purely so the orchestrator can stream progress events to the
/// UI without breaking the *_many abstraction.
pub type ProgressTick = Arc<dyn Fn() + Send + Sync>;

/// Verify many findings in parallel. Caps concurrency at `concurrency` (use
/// `DEFAULT_VERIFY_CONCURRENCY` for the CLAUDE.md default). Hardening
/// findings skip the LLM and pass through with `verdict: None`. Failures on
/// individual findings are logged and surface as `verdict: None`.
///
/// `on_item` is invoked once per completed finding (regardless of
/// success/failure/hardening). Pass `None` to opt out — the test suite does.
pub async fn verify_many(
    findings: Vec<Finding>,
    scan_root: PathBuf,
    provider: Arc<dyn Provider>,
    model: &str,
    concurrency: usize,
    on_item: Option<ProgressTick>,
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
            if let Some(tick) = &on_item {
                tick();
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ProviderResult;
    use crate::providers::{ContentBlock, GenerationRequest, Response, StopReason, Usage};
    use crate::scanner::Severity;
    use async_trait::async_trait;
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
    }

    #[test]
    fn submit_verdict_tool_is_openai_strict_compatible() {
        crate::providers::test_support::assert_openai_strict_compatible(
            &submit_verdict_tool().input_schema,
        );
    }

    fn submit_verdict_response(verdict_json: Value) -> Response {
        Response {
            id: "msg_final".into(),
            model: "scripted".into(),
            content: vec![ContentBlock::ToolUse {
                id: "toolu_test".into(),
                name: "submit_verdict".into(),
                input: verdict_json,
            }],
            stop_reason: Some(StopReason::ToolUse),
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

        let provider = ScriptedProvider::new(vec![submit_verdict_response(json!({
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
        }))]);

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

        let provider = ScriptedProvider::new(vec![submit_verdict_response(json!({
            "is_reachable": false,
            "source_is_untrusted": true,
            "concrete_exploit": null,
            "reasoning": "the sink is gated by a prior check the detection pass missed"
        }))]);
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
        let provider: Arc<dyn Provider> = Arc::new(ScriptedProvider::new(vec![submit_verdict_response(json!({
            "is_reachable": true,
            "source_is_untrusted": true,
            "concrete_exploit": {"kind": "other", "payload": "x", "expected_effect": "y"},
            "reasoning": "ok"
        }))]));

        let findings = vec![
            mk_finding("focus.ts", FindingKind::Hardening),
            mk_finding("focus.ts", FindingKind::Vuln),
            mk_finding("focus.ts", FindingKind::Hardening),
        ];
        let out = verify_many(findings, root.clone(), provider, "scripted", 2, None).await;
        assert_eq!(out.len(), 3);
        assert_eq!(out.iter().filter(|v| v.verdict.is_some()).count(), 1);
        assert_eq!(out.iter().filter(|v| v.verdict.is_none()).count(), 2);
    }
}
