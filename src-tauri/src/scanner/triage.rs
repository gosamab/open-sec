//! Triage pass — a Haiku gate that buckets each candidate file into
//! { high, normal, low, skip }. Detect runs on everything except `skip`,
//! ordered by bucket. Test files default to `low`; configs/generated/snapshot
//! files default to `skip`; auth/IO/parsing/DB-heavy code goes `high`.

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{debug, instrument, warn};

use crate::providers::{
    CacheControl, ContentBlock, GenerationRequest, Message, Provider, Role, SystemBlock, Tool,
};
use crate::scanner::ingest::Candidate;

pub const DEFAULT_TRIAGE_MODEL: &str = "claude-haiku-4-5";
pub const DEFAULT_TRIAGE_CONCURRENCY: usize = 8;
const MAX_TOKENS: u32 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    High,
    Normal,
    Low,
    Skip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriageResult {
    pub priority: Priority,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriagedFile {
    pub candidate: Candidate,
    pub result: TriageResult,
}

const TRIAGE_PROMPT: &str = r#"You are a fast triage filter for a security code-review pipeline. Your job
is to BUCKET each file into one of four priorities so the expensive detection
stage focuses on code most likely to contain real vulnerabilities.

You DO NOT identify vulnerabilities here. You only decide whether the
detection model should look at this file, and how soon.

Call the `submit_triage` tool with your decision. That tool call IS your
final answer.

BUCKETS

high   — Code that handles external input or crosses a trust boundary:
         HTTP/RPC handlers, request parsers, auth/session logic, DB query
         builders, command/process execution, file/path operations, network
         clients (incl. webhook receivers), deserializers, template engines,
         crypto/secret handling, IPC entry points. Anything where a clear
         source → sink could plausibly exist.

normal — First-party application/library code that is not obviously a
         trust boundary but could still hide bugs: domain logic, internal
         APIs, data transformations, business-rule code, utilities that
         touch the above categories indirectly.

low    — RESERVED for test files (unit/integration/e2e), example/demo code,
         fixtures, mocks, storybook/playground files. Do not use `low` for
         any other reason. A file that has no apparent trust boundary but is
         real first-party application/library code is `normal`, NOT `low`.

skip   — Files with no meaningful executable security surface:
         generated code (marked as such, or matching a clear generator
         pattern), pure type/interface declarations with no logic, lockfile
         contents, vendored copies that slipped past the directory filter,
         snapshot files, files that are entirely comments/data, empty
         scaffolds, build configs that just re-export framework defaults.

RULES

- Be DECISIVE. Pick exactly one bucket. Do not waffle.
- "I don't know what this does" defaults to `normal`, not `skip` — only
  `skip` for files that are clearly inert.
- Test files default to `low` even if they exercise risky APIs (the test
  exists because the prod code exists; we'll find the bug in the prod code).
  Promote a test to `normal` only if it appears to contain real logic that
  isn't being tested elsewhere.
- A pure-logic file with no I/O and no external input is `normal`, never
  `low`. Pure-logic bugs (off-by-one, wrong sign, mis-applied discount) are
  in scope for detection; the only reason `low` exists is to drain the
  queue of test/example files last.
- Generated code (e.g. "// @generated", "DO NOT EDIT", protobuf/openapi/
  prisma output) → `skip`.
- `reason` must be a short concrete justification grounded in what you saw
  in the file, not a restatement of the bucket definition."#;

fn submit_triage_tool() -> Tool {
    Tool {
        name: "submit_triage".to_string(),
        description: "Submit your triage bucket for the file.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "priority": { "type": "string", "enum": ["high", "normal", "low", "skip"] },
                "reason":   { "type": "string", "description": "Short justification (<= 200 chars)." }
            },
            "required": ["priority", "reason"]
        }),
        // Triage is one-shot with a single tool; the cache marker lives here
        // so the system prompt + tool block cache together.
        cache_control: Some(CacheControl::ephemeral_1h()),
    }
}

/// Triage a single file. One-shot: no read tools, no agent loop — the model
/// must call `submit_triage` immediately.
#[instrument(skip(provider, source), fields(path = %rel_path, bytes = source.len()))]
pub async fn triage_one(
    rel_path: &str,
    source: &str,
    provider: &dyn Provider,
    model: &str,
) -> Result<TriageResult> {
    let mut req = GenerationRequest::new(model, MAX_TOKENS);
    req.temperature = Some(0.0);
    req.system
        .push(SystemBlock::text(TRIAGE_PROMPT).with_cache(CacheControl::ephemeral_1h()));
    req.tools = vec![submit_triage_tool()];
    req.messages.push(Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: format!("File: {rel_path}\n\n{source}"),
        }],
    });

    let resp = provider
        .generate(req)
        .await
        .context("anthropic generate call failed")?;
    debug!(blocks = resp.content.len(), "received triage response");

    let input = resp
        .content
        .iter()
        .find_map(|b| match b {
            ContentBlock::ToolUse { name, input, .. } if name == "submit_triage" => Some(input),
            _ => None,
        })
        .ok_or_else(|| anyhow!("triage response did not call submit_triage"))?;

    serde_json::from_value(input.clone()).context("submit_triage input did not match schema")
}

/// Triage every candidate in parallel under a semaphore. Errors on individual
/// files are logged and the file is dropped from the result; we don't fail the
/// whole scan because one file's triage call hiccupped.
pub async fn triage_many(
    candidates: Vec<Candidate>,
    provider: Arc<dyn Provider>,
    model: &str,
    concurrency: usize,
) -> Vec<TriagedFile> {
    let permits = Arc::new(Semaphore::new(concurrency.max(1)));
    let model = model.to_string();
    let mut set: JoinSet<Option<TriagedFile>> = JoinSet::new();

    for candidate in candidates {
        let permits = permits.clone();
        let provider = provider.clone();
        let model = model.clone();
        set.spawn(async move {
            let _permit = permits.acquire_owned().await.ok()?;
            let source = match tokio::fs::read_to_string(&candidate.path).await {
                Ok(s) => s,
                Err(e) => {
                    warn!(path = %candidate.rel_path, error = %e, "skipping: read failed");
                    return None;
                }
            };
            match triage_one(&candidate.rel_path, &source, provider.as_ref(), &model).await {
                Ok(result) => Some(TriagedFile { candidate, result }),
                Err(e) => {
                    warn!(path = %candidate.rel_path, error = %e, "triage call failed");
                    None
                }
            }
        });
    }

    let mut out = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok(Some(t)) = joined {
            out.push(t);
        }
    }
    // Stable ordering by priority bucket then path, so downstream queue order
    // is deterministic.
    out.sort_by(|a, b| {
        priority_rank(a.result.priority)
            .cmp(&priority_rank(b.result.priority))
            .then_with(|| a.candidate.rel_path.cmp(&b.candidate.rel_path))
    });
    out
}

fn priority_rank(p: Priority) -> u8 {
    match p {
        Priority::High => 0,
        Priority::Normal => 1,
        Priority::Low => 2,
        Priority::Skip => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ProviderResult;
    use crate::providers::{Response, StopReason, Usage};
    use async_trait::async_trait;
    use tempfile::TempDir;

    struct FixedProvider {
        input: serde_json::Value,
    }

    impl FixedProvider {
        fn new(input: serde_json::Value) -> Self {
            Self { input }
        }
    }

    #[async_trait]
    impl Provider for FixedProvider {
        fn name(&self) -> &'static str {
            "fixed"
        }
        async fn generate(&self, _req: GenerationRequest) -> ProviderResult<Response> {
            Ok(Response {
                id: "msg".into(),
                model: "fixed".into(),
                content: vec![ContentBlock::ToolUse {
                    id: "toolu_triage".into(),
                    name: "submit_triage".into(),
                    input: self.input.clone(),
                }],
                stop_reason: Some(StopReason::ToolUse),
                stop_sequence: None,
                usage: Usage::default(),
            })
        }
    }

    #[tokio::test]
    async fn triage_one_parses_bucket_and_reason() {
        let provider = FixedProvider::new(
            json!({"priority":"high","reason":"http handler reads body, queries DB"}),
        );
        let r = triage_one("src/login.ts", "// code", &provider, "fixed")
            .await
            .unwrap();
        assert_eq!(r.priority, Priority::High);
        assert!(r.reason.contains("http handler"));
    }

    #[tokio::test]
    async fn triage_many_sorts_by_priority_then_path() {
        let tmp = TempDir::new().unwrap();
        let mk = |name: &str| {
            let p = tmp.path().join(name);
            std::fs::write(&p, b"x").unwrap();
            Candidate {
                path: p,
                rel_path: name.to_string(),
                size_bytes: 1,
                line_count: 1,
            }
        };
        let provider = Arc::new(FixedProvider::new(json!({"priority":"normal","reason":"ok"})));
        let out = triage_many(
            vec![mk("z.ts"), mk("a.ts"), mk("m.ts")],
            Arc::clone(&provider) as Arc<dyn Provider>,
            "fixed",
            4,
        )
        .await;
        let names: Vec<_> = out.iter().map(|t| t.candidate.rel_path.clone()).collect();
        assert_eq!(names, vec!["a.ts", "m.ts", "z.ts"]);
    }
}
