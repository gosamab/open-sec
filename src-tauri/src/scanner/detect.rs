use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;
use tracing::instrument;

use crate::providers::{Provider, Tool};
use crate::scanner::agent_loop::{run_agent_loop, AgentRequest};
use crate::scanner::util::with_line_numbers;
use crate::scanner::Finding;
use crate::tools;

pub const DEFAULT_DETECT_MODEL: &str = "claude-sonnet-4-6";
const DEFAULT_MAX_TOKENS: u32 = 8192;

const BASE_DETECTION_PROMPT: &str = r#"You are a meticulous application-security reviewer.

You identify two classes of issues in source code:
1. "vuln"      — concrete vulnerabilities with a describable source → sink path.
2. "hardening" — defense-in-depth opportunities that do not claim an exploitable bug.

The `submit_findings` tool enforces the JSON shape; these are the semantics:

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
- If you find nothing, call `submit_findings` with an empty array."#;

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

When you're done investigating, call `submit_findings` with the structured
findings array. That tool call IS your final answer.
"#;

#[derive(Deserialize)]
struct FindingsEnvelope {
    findings: Vec<Finding>,
}

fn submit_findings_tool() -> Tool {
    Tool {
        name: "submit_findings".to_string(),
        description: "Submit your final findings array for the focus file. Call this exactly \
                      once when you're done investigating. Pass an empty array if the file is clean."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "findings": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "kind":        { "type": "string", "enum": ["vuln", "hardening"] },
                            "severity":    { "type": "string", "enum": ["critical","high","medium","low","info"] },
                            "cwe":         { "type": "string", "description": "CWE-<number>, e.g. CWE-89" },
                            "owasp":       { "type": ["string", "null"], "description": "A<NN>:<year>, e.g. A03:2021" },
                            "title":       { "type": "string", "description": "One-line summary, <= 80 chars" },
                            "file":        { "type": "string", "description": "Repeat the focus-file path verbatim" },
                            "line_start":  { "type": "integer", "minimum": 1 },
                            "line_end":    { "type": "integer", "minimum": 1 },
                            "description": { "type": "string", "description": "2-4 sentences" },
                            "data_flow":   { "type": "string", "description": "source → transformation → sink narrative" }
                        },
                        "required": ["kind","severity","cwe","title","file","line_start","line_end","description","data_flow"]
                    }
                }
            },
            "required": ["findings"]
        }),
        cache_control: None,
    }
}

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

    let initial_user_msg = format!(
        "Focus file: {file_label}\n\nThe scan root is {}. \
         You may use the provided tools to read other files under that root.\n\n\
         Here is the focus file with line numbers:\n\n{}",
        canonical_root.display(),
        with_line_numbers(source),
    );

    let tool_input = run_agent_loop(
        AgentRequest {
            system_prompt: format!("{TOOLS_PREAMBLE}\n{BASE_DETECTION_PROMPT}"),
            initial_user_msg,
            model,
            max_tokens: DEFAULT_MAX_TOKENS,
            temperature: Some(0.0),
            canonical_root: &canonical_root,
            provider,
            stage_label: "detect",
        },
        submit_findings_tool(),
    )
    .await?;

    let envelope: FindingsEnvelope = serde_json::from_value(tool_input)
        .context("submit_findings input did not match schema")?;
    let mut findings = envelope.findings;
    for f in &mut findings {
        // The model is told to repeat the focus path verbatim, but normalize
        // anyway so the stable id is computed from a known value.
        f.file = file_label.clone();
        f.assign_id();
    }
    Ok(findings)
}

#[cfg(test)]
mod agent_tests {
    use super::*;
    use crate::error::ProviderResult;
    use crate::providers::{
        ContentBlock, GenerationRequest, Provider, Response, Role, StopReason, Usage,
    };
    use crate::scanner::agent_loop::MAX_TOOL_ITERATIONS;
    use async_trait::async_trait;
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
            tool_use_response(
                "toolu_2",
                "submit_findings",
                json!({"findings":[{
                    "kind":"vuln","severity":"high","cwe":"CWE-89","owasp":null,
                    "title":"Test finding","file":"focus.ts","line_start":1,"line_end":2,
                    "description":"d","data_flow":"src→sink"
                }]}),
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

