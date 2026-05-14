use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use tracing::{debug, info, instrument, warn};

use crate::providers::{
    CacheControl, ContentBlock, GenerationRequest, Message, Provider, Role, StopReason,
    SystemBlock,
};
use crate::scanner::util::{collect_text, extract_json_object};
use crate::scanner::Finding;
use crate::tools;

pub const DEFAULT_DETECT_MODEL: &str = "claude-sonnet-4-6";
const DEFAULT_MAX_TOKENS: u32 = 8192;
const MAX_TOOL_ITERATIONS: usize = 25;

/// Tool-agnostic core of the detection prompt: role, output schema, severity
/// guide, and rules. Used verbatim in no-tools mode and prefixed with
/// `TOOLS_PREAMBLE` in agent-loop mode.
const BASE_DETECTION_PROMPT: &str = r#"You are a meticulous application-security reviewer.

You identify two classes of issues in source code:
1. "vuln"      — concrete vulnerabilities with a describable source → sink path.
2. "hardening" — defense-in-depth opportunities that do not claim an exploitable bug.

OUTPUT FORMAT

Output STRICT JSON only — no prose, no markdown fences, no commentary.
The top-level object must be: {"findings": [ ... ]}.

Each finding MUST contain every field below:
  kind:        "vuln" | "hardening"
  severity:    "critical" | "high" | "medium" | "low" | "info"
  cwe:         "CWE-<number>"        (required; pick the best-fit CWE)
  owasp:       "A<NN>:<year>" | null (optional OWASP Top 10 mapping)
  title:       <one-line summary, <= 80 chars>
  file:        <repeat the focus-file path from the user message verbatim>
  line_start:  <integer, 1-indexed>
  line_end:    <integer, 1-indexed, inclusive>
  description: <2–4 sentences explaining the issue>
  data_flow:   <source → transformation → sink narrative>

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
- line_start / line_end must reference real lines in the focus file. Prefer the
  minimal span that contains the source → sink, not the whole enclosing function.
- If you find nothing, return {"findings": []}.
- Do NOT add fields not listed above. Do NOT wrap the JSON in markdown."#;

/// Prepended to `BASE_DETECTION_PROMPT` in agent-loop mode. Explains the
/// available tools and the conversational protocol (final message must be
/// JSON, no trailing tool calls).
const TOOLS_PREAMBLE: &str = r#"You have read-only access to the project under review through these tools:
  - list_imports — learn what the focus file depends on
  - read_file / read_file_range — read related files
  - grep — search by regex/literal across the project
  - find_references — locate callers/users of a function or symbol
  - list_directory — orient yourself in the project layout
  - git_blame — only when authorship/age would change your conclusion

The user message names ONE focus file. Findings MUST be about issues in that
focus file. Use tools only to gather context that materially changes your
conclusion. Each tool call costs tokens and latency — be parsimonious. Stop
calling tools as soon as you have what you need.

Your FINAL assistant message MUST be the JSON object alone. No tool calls in
that final message; no prose other than the JSON.
"#;

fn system_prompt(with_tools: bool) -> String {
    if with_tools {
        format!("{TOOLS_PREAMBLE}\n{BASE_DETECTION_PROMPT}")
    } else {
        BASE_DETECTION_PROMPT.to_string()
    }
}

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
        SystemBlock::text(system_prompt(false)).with_cache(CacheControl::ephemeral_1h()),
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

/// Agentic scan: gives the model tool access (read_file, grep, find_references,
/// list_directory, list_imports, git_blame) scoped to `scan_root`, runs the
/// agent loop, and parses the final JSON.
#[instrument(skip(provider, source), fields(path = %file_path.display(), root = %scan_root.display(), bytes = source.len()))]
pub async fn scan_with_tools(
    file_path: &Path,
    scan_root: &Path,
    source: &str,
    provider: &dyn Provider,
    model: &str,
) -> Result<Vec<Finding>> {
    let canonical_root = tools::sandbox::canonical_root(scan_root)?;
    let file_label = file_path.display().to_string();

    let initial = format!(
        "Focus file: {file_label}\n\nThe scan root is {}. \
         You may use the provided tools to read other files under that root.\n\n\
         Here is the focus file with line numbers:\n\n{}",
        canonical_root.display(),
        with_line_numbers(source),
    );

    let mut messages: Vec<Message> = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text { text: initial }],
    }];

    let tool_defs = tools::tool_definitions();

    for iteration in 0..MAX_TOOL_ITERATIONS {
        let mut req = GenerationRequest::new(model, DEFAULT_MAX_TOKENS);
        req.temperature = Some(0.0);
        req.system.push(
            SystemBlock::text(system_prompt(true)).with_cache(CacheControl::ephemeral_1h()),
        );
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
            "detect iteration"
        );

        // Always append the assistant's response so that any tool_use blocks
        // are referenced by their tool_use_id in the next turn.
        messages.push(Message {
            role: Role::Assistant,
            content: resp.content.clone(),
        });

        let tool_uses: Vec<(String, String, serde_json::Value)> = resp
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
            return finalize(&text, &file_label);
        }

        info!(
            iteration,
            tool_calls = tool_uses.len(),
            "executing tool calls"
        );

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
        "hit the {MAX_TOOL_ITERATIONS}-iteration tool-use cap without a final answer"
    ))
}

fn finalize(text: &str, file_label: &str) -> Result<Vec<Finding>> {
    let json = extract_json_object(text)
        .ok_or_else(|| anyhow!("model response did not contain a JSON object: {text}"))?;
    let envelope: FindingsEnvelope =
        serde_json::from_str(json).with_context(|| format!("parsing findings JSON: {json}"))?;
    let mut findings = envelope.findings;
    for f in &mut findings {
        f.file = file_label.to_string();
        f.assign_id();
    }
    Ok(findings)
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
mod agent_tests {
    use super::*;
    use crate::error::ProviderResult;
    use crate::providers::{Provider, Response, StreamEvent, Usage};
    use async_trait::async_trait;
    use futures::stream::BoxStream;
    use serde_json::json;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Minimal `Provider` that returns scripted responses in order. Used to
    /// drive the agent loop deterministically without hitting the real API.
    struct ScriptedProvider {
        responses: Mutex<Vec<Response>>,
        received: Mutex<Vec<GenerationRequest>>,
    }

    impl ScriptedProvider {
        fn new(responses: Vec<Response>) -> Self {
            Self {
                responses: Mutex::new(responses),
                received: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl Provider for ScriptedProvider {
        fn name(&self) -> &'static str {
            "scripted"
        }
        async fn generate(&self, req: GenerationRequest) -> ProviderResult<Response> {
            self.received.lock().unwrap().push(req);
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
            unimplemented!("not used in agent tests");
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

    fn tool_use_response(id: &str, name: &str, input: serde_json::Value) -> Response {
        Response {
            id: format!("msg_{id}"),
            model: "scripted".into(),
            content: vec![ContentBlock::ToolUse {
                id: id.into(),
                name: name.into(),
                input,
            }],
            stop_reason: Some(StopReason::ToolUse),
            stop_sequence: None,
            usage: Usage::default(),
        }
    }

    #[tokio::test]
    async fn agent_loop_dispatches_tool_then_parses_findings() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::write(root.join("focus.ts"), "// vulnerable\nconst x = 1;\n").unwrap();
        std::fs::write(root.join("helper.ts"), "export const y = 2;\n").unwrap();

        let provider = ScriptedProvider::new(vec![
            tool_use_response("toolu_1", "read_file", json!({"path": "helper.ts"})),
            text_response(
                r#"{"findings":[{"kind":"vuln","severity":"high","cwe":"CWE-89","owasp":null,"title":"Test finding","file":"focus.ts","line_start":1,"line_end":2,"description":"d","data_flow":"src→sink"}]}"#,
            ),
        ]);

        let findings = scan_with_tools(
            &root.join("focus.ts"),
            &root,
            "// vulnerable\nconst x = 1;\n",
            &provider,
            "scripted",
        )
        .await
        .unwrap();

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].cwe, "CWE-89");
        assert_eq!(findings[0].title, "Test finding");
        // Stable id was computed server-side, not from the model.
        assert!(!findings[0].id.is_empty());

        // The provider should have seen 2 requests:
        // - initial user message (no assistant turns yet)
        // - assistant tool_use + our tool_result, asking for the final answer
        let received = provider.received.lock().unwrap();
        assert_eq!(received.len(), 2);

        // Second request must contain a tool_result block referencing toolu_1.
        let second = &received[1];
        let last_user = second.messages.last().unwrap();
        assert!(matches!(last_user.role, Role::User));
        let has_result = last_user.content.iter().any(|b| {
            matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "toolu_1")
        });
        assert!(has_result, "second request should include a tool_result for toolu_1");
    }

    #[tokio::test]
    async fn agent_loop_hits_iteration_cap() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::write(root.join("focus.ts"), "x\n").unwrap();

        // 26 consecutive tool calls — should bail out after MAX_TOOL_ITERATIONS.
        let mut responses = Vec::new();
        for i in 0..MAX_TOOL_ITERATIONS + 1 {
            responses.push(tool_use_response(
                &format!("toolu_{i}"),
                "read_file",
                json!({"path": "focus.ts"}),
            ));
        }
        let provider = ScriptedProvider::new(responses);

        let err = scan_with_tools(
            &root.join("focus.ts"),
            &root,
            "x\n",
            &provider,
            "scripted",
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("iteration"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_numbers_pad_to_width() {
        let out = with_line_numbers("a\nb\nc");
        // 3 lines → width 3
        assert!(out.starts_with("  1| a\n"));
        assert!(out.contains("  2| b\n"));
        assert!(out.contains("  3| c"));
    }
}
