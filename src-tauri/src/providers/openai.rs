//! OpenAI Chat Completions provider.
//!
//! Surfaces OpenAI's gpt-5 family behind the same `Provider` trait as
//! `AnthropicProvider`, with four asymmetries handled at the boundary:
//!   - Content-block messages → flat Chat Completions messages. The
//!     translator splits Anthropic's batched-tool-result user messages into
//!     individual `{role: "tool"}` entries that OpenAI requires.
//!   - JSON-schema strict mode is opt-in per-tool (only `submit_*` tools);
//!     read-only tools stay non-strict because they have optional fields.
//!   - `temperature` is dropped on the wire for gpt-5 models, which reject
//!     anything but the default.
//!   - `reasoning_effort` is pinned to `"minimal"` for gpt-5 models. The
//!     per-stage `max_completion_tokens` budgets in the scanner are sized
//!     for Anthropic (output-only); on gpt-5 the same field caps reasoning
//!     + visible output, so default reasoning silently eats the budget
//!     before any tool call lands — files then drop out of triage and the
//!     scan reports clean.
//! Caching is automatic (no `cache_control` knob): we read back the cached
//! token count from `usage.prompt_tokens_details.cached_tokens` and feed it
//! into the existing `Usage` accounting so cost reporting stays uniform.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use reqwest::{header, Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, instrument};

use crate::error::{ProviderError, ProviderResult};
use crate::providers::rate_limit::{MultiObserver, RateLimitSnapshot};
use crate::providers::{
    ContentBlock, GenerationRequest, Message, Provider, Response, Role, StopReason, SystemBlock,
    Tool, Usage,
};

const DEFAULT_BASE_URL: &str = "https://api.openai.com";
const PROVIDER_KEY: &str = "openai";

pub struct OpenAiProvider {
    api_key: String,
    base_url: String,
    client: Client,
    observer: Option<Arc<MultiObserver>>,
}

impl OpenAiProvider {
    pub fn new(api_key: impl Into<String>) -> ProviderResult<Self> {
        Self::with_base_url(api_key, DEFAULT_BASE_URL)
    }

    pub fn with_base_url(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
    ) -> ProviderResult<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(600))
            .build()?;
        Ok(Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            client,
            observer: None,
        })
    }

    pub fn with_rate_limit_observer(mut self, observer: Arc<MultiObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    fn chat_url(&self) -> String {
        format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        )
    }
}

// ============ Wire types — request ============

#[derive(Serialize, Debug)]
struct ChatRequest {
    model: String,
    max_completion_tokens: u32,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ChatTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'static str>,
    stream: bool,
}

#[derive(Serialize, Debug, PartialEq)]
struct ChatMessage {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

impl ChatMessage {
    fn system(content: String) -> Self {
        Self {
            role: "system",
            content: Some(content),
            tool_calls: None,
            tool_call_id: None,
        }
    }
    fn user(content: String) -> Self {
        Self {
            role: "user",
            content: Some(content),
            tool_calls: None,
            tool_call_id: None,
        }
    }
    fn assistant(content: Option<String>, tool_calls: Option<Vec<ToolCall>>) -> Self {
        Self {
            role: "assistant",
            content,
            tool_calls,
            tool_call_id: None,
        }
    }
    fn tool(tool_call_id: String, content: String) -> Self {
        Self {
            role: "tool",
            content: Some(content),
            tool_calls: None,
            tool_call_id: Some(tool_call_id),
        }
    }
}

#[derive(Serialize, Debug, PartialEq)]
struct ToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
    function: FunctionCall,
}

#[derive(Serialize, Debug, PartialEq)]
struct FunctionCall {
    name: String,
    /// JSON-encoded string of the tool's arguments — NOT a JSON object.
    arguments: String,
}

#[derive(Serialize, Debug)]
struct ChatTool {
    #[serde(rename = "type")]
    kind: &'static str,
    function: ChatToolFunction,
}

#[derive(Serialize, Debug)]
struct ChatToolFunction {
    name: String,
    description: String,
    parameters: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    strict: Option<bool>,
}

// ============ Wire types — response ============

#[derive(Deserialize, Debug)]
struct ChatResponse {
    id: String,
    model: String,
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: ChatUsage,
}

#[derive(Deserialize, Debug)]
struct ChatChoice {
    message: ChatChoiceMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
struct ChatChoiceMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCallResp>>,
}

#[derive(Deserialize, Debug)]
struct ToolCallResp {
    id: String,
    function: FunctionCallResp,
}

#[derive(Deserialize, Debug)]
struct FunctionCallResp {
    name: String,
    arguments: String,
}

#[derive(Default, Deserialize, Debug)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
    #[serde(default)]
    prompt_tokens_details: Option<PromptTokensDetails>,
}

#[derive(Default, Deserialize, Debug)]
struct PromptTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u32>,
}

// ============ Translation ============

fn to_chat_messages(
    system: &[SystemBlock],
    messages: &[Message],
) -> ProviderResult<Vec<ChatMessage>> {
    let mut out = Vec::new();

    if !system.is_empty() {
        let combined = system
            .iter()
            .map(|s| s.text.clone())
            .collect::<Vec<_>>()
            .join("\n\n");
        out.push(ChatMessage::system(combined));
    }

    for m in messages {
        match m.role {
            Role::User => {
                let mut texts: Vec<String> = Vec::new();
                let mut tool_results: Vec<(String, String)> = Vec::new();
                for block in &m.content {
                    match block {
                        ContentBlock::Text { text } => texts.push(text.clone()),
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
                            let payload = if *is_error {
                                format!("[tool error] {content}")
                            } else {
                                content.clone()
                            };
                            tool_results.push((tool_use_id.clone(), payload));
                        }
                        ContentBlock::ToolUse { .. } => {
                            return Err(ProviderError::BadRequest(
                                "ToolUse block in user message — invariant violation".into(),
                            ));
                        }
                    }
                }
                if !texts.is_empty() {
                    out.push(ChatMessage::user(texts.join("\n\n")));
                }
                for (id, content) in tool_results {
                    out.push(ChatMessage::tool(id, content));
                }
            }
            Role::Assistant => {
                let mut text = String::new();
                let mut calls: Vec<ToolCall> = Vec::new();
                for block in &m.content {
                    match block {
                        ContentBlock::Text { text: t } => {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(t);
                        }
                        ContentBlock::ToolUse { id, name, input } => {
                            let arguments = serde_json::to_string(input).map_err(|e| {
                                ProviderError::BadRequest(format!(
                                    "tool_use input not serializable: {e}"
                                ))
                            })?;
                            calls.push(ToolCall {
                                id: id.clone(),
                                kind: "function",
                                function: FunctionCall {
                                    name: name.clone(),
                                    arguments,
                                },
                            });
                        }
                        ContentBlock::ToolResult { .. } => {
                            return Err(ProviderError::BadRequest(
                                "ToolResult block in assistant message — invariant violation"
                                    .into(),
                            ));
                        }
                    }
                }
                out.push(ChatMessage::assistant(
                    if text.is_empty() { None } else { Some(text) },
                    if calls.is_empty() { None } else { Some(calls) },
                ));
            }
        }
    }

    Ok(out)
}

fn to_chat_tools(tools: &[Tool]) -> Vec<ChatTool> {
    tools
        .iter()
        .map(|t| ChatTool {
            kind: "function",
            function: ChatToolFunction {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.input_schema.clone(),
                strict: if t.name.starts_with("submit_") {
                    Some(true)
                } else {
                    None
                },
            },
        })
        .collect()
}

fn to_usage(u: &ChatUsage) -> Usage {
    let cached = u
        .prompt_tokens_details
        .as_ref()
        .and_then(|d| d.cached_tokens)
        .unwrap_or(0);
    Usage {
        input_tokens: u.prompt_tokens.saturating_sub(cached),
        output_tokens: u.completion_tokens,
        cache_read_input_tokens: cached,
        cache_creation_input_tokens: 0,
    }
}

fn from_chat_response(resp: ChatResponse) -> ProviderResult<Response> {
    let usage = to_usage(&resp.usage);
    let choice = resp
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| ProviderError::BadRequest("OpenAI response had no choices".into()))?;

    let mut content: Vec<ContentBlock> = Vec::new();
    if let Some(text) = choice.message.content.filter(|s| !s.is_empty()) {
        content.push(ContentBlock::Text { text });
    }
    if let Some(calls) = choice.message.tool_calls {
        for c in calls {
            let input: Value = serde_json::from_str(&c.function.arguments).map_err(|e| {
                ProviderError::BadRequest(format!("OpenAI tool_call arguments not JSON: {e}"))
            })?;
            content.push(ContentBlock::ToolUse {
                id: c.id,
                name: c.function.name,
                input,
            });
        }
    }

    // A `length` finish with no content at all is the classic "reasoning ate
    // the budget" failure on gpt-5. Returning an empty `Response` would
    // silently propagate as "model didn't call the tool" upstream; surface
    // it as an explicit BadRequest so the scanner shows a real error.
    if matches!(choice.finish_reason.as_deref(), Some("length")) && content.is_empty() {
        return Err(ProviderError::BadRequest(
            "OpenAI response truncated at max_completion_tokens with no content — \
             likely reasoning consumed the budget. Try a smaller reasoning_effort \
             or a larger max_tokens."
                .into(),
        ));
    }

    let stop_reason = match choice.finish_reason.as_deref() {
        Some("tool_calls") => Some(StopReason::ToolUse),
        Some("length") => Some(StopReason::MaxTokens),
        Some("stop") => Some(StopReason::EndTurn),
        _ => None,
    };

    Ok(Response {
        id: resp.id,
        model: resp.model,
        content,
        stop_reason,
        stop_sequence: None,
        usage,
    })
}

// ============ Provider impl ============

#[async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &'static str {
        PROVIDER_KEY
    }

    #[instrument(skip(self, req), fields(provider = "openai", model = %req.model))]
    async fn generate(&self, req: GenerationRequest) -> ProviderResult<Response> {
        let messages = to_chat_messages(&req.system, &req.messages)?;
        let tools = to_chat_tools(&req.tools);
        let tool_choice = if tools.is_empty() { None } else { Some("required") };
        let is_gpt5 = req.model.starts_with("gpt-5");
        let temperature = if is_gpt5 { None } else { req.temperature };
        // gpt-5 reasoning + the scanner's Anthropic-sized output budgets don't
        // mix — see the module docstring. "minimal" lets the model spend
        // almost the entire budget on the tool-call output.
        let reasoning_effort = if is_gpt5 { Some("minimal") } else { None };

        let body = ChatRequest {
            model: req.model.clone(),
            max_completion_tokens: req.max_tokens,
            messages,
            tools,
            tool_choice,
            temperature,
            reasoning_effort,
            stream: false,
        };

        let http_resp = self
            .client
            .post(self.chat_url())
            .bearer_auth(&self.api_key)
            .header(header::CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await?;

        if let Some(observer) = &self.observer {
            observer.record(
                PROVIDER_KEY,
                RateLimitSnapshot::from_openai_headers(http_resp.headers(), SystemTime::now()),
            );
        }

        if !http_resp.status().is_success() {
            return Err(classify_error(http_resp).await);
        }

        let parsed: ChatResponse = http_resp.json().await?;
        debug!(
            input_tokens = parsed.usage.prompt_tokens,
            output_tokens = parsed.usage.completion_tokens,
            cached = parsed
                .usage
                .prompt_tokens_details
                .as_ref()
                .and_then(|d| d.cached_tokens)
                .unwrap_or(0),
            "openai generate finished"
        );
        from_chat_response(parsed)
    }
}

#[derive(Deserialize)]
struct OpenAiErrorBody {
    error: OpenAiErrorInner,
}

#[derive(Deserialize)]
struct OpenAiErrorInner {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    message: String,
}

async fn classify_error(resp: reqwest::Response) -> ProviderError {
    let status = resp.status();
    let retry_after = parse_retry_after(&resp);
    let body = resp.text().await.unwrap_or_default();

    let parsed_message = serde_json::from_str::<OpenAiErrorBody>(&body)
        .ok()
        .map(|e| match e.error.kind {
            Some(kind) => format!("{kind}: {}", e.error.message),
            None => e.error.message,
        });
    let message = parsed_message.unwrap_or_else(|| body.clone());

    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => ProviderError::AuthFailed,
        StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimited { retry_after },
        s if s.is_client_error() => ProviderError::BadRequest(message),
        s => ProviderError::Server {
            status: s.as_u16(),
            body: message,
        },
    }
}

fn parse_retry_after(resp: &reqwest::Response) -> Option<Duration> {
    resp.headers()
        .get(header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{CacheControl, ContentBlock, Message, Role, SystemBlock};
    use serde_json::{json, Value};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // ---- translator tests ----

    #[test]
    fn translate_system_blocks_concat() {
        let system = vec![
            SystemBlock::text("rules part 1"),
            SystemBlock::text("rules part 2").with_cache(CacheControl::ephemeral_1h()),
        ];
        let out = to_chat_messages(&system, &[]).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, "system");
        assert_eq!(out[0].content.as_deref(), Some("rules part 1\n\nrules part 2"));
    }

    #[test]
    fn translate_user_text_only() {
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "scan this".into(),
            }],
        }];
        let out = to_chat_messages(&[], &messages).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, "user");
        assert_eq!(out[0].content.as_deref(), Some("scan this"));
        assert!(out[0].tool_call_id.is_none());
    }

    #[test]
    fn translate_assistant_with_text_and_tool_use() {
        let messages = vec![Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "Looking up helper.ts".into(),
                },
                ContentBlock::ToolUse {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    input: json!({"path": "helper.ts"}),
                },
                ContentBlock::ToolUse {
                    id: "call_2".into(),
                    name: "grep".into(),
                    input: json!({"pattern": "exec"}),
                },
            ],
        }];
        let out = to_chat_messages(&[], &messages).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, "assistant");
        assert_eq!(out[0].content.as_deref(), Some("Looking up helper.ts"));
        let calls = out[0].tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].kind, "function");
        assert_eq!(calls[0].function.name, "read_file");
        // arguments must be a JSON-encoded STRING, not a nested object.
        let parsed: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(parsed, json!({"path": "helper.ts"}));
        assert_eq!(calls[1].function.name, "grep");
    }

    #[test]
    fn translate_user_batched_tool_results_splits() {
        let messages = vec![Message {
            role: Role::User,
            content: vec![
                ContentBlock::ToolResult {
                    tool_use_id: "call_1".into(),
                    content: "file contents".into(),
                    is_error: false,
                },
                ContentBlock::ToolResult {
                    tool_use_id: "call_2".into(),
                    content: "match found".into(),
                    is_error: false,
                },
            ],
        }];
        let out = to_chat_messages(&[], &messages).unwrap();
        // One {role:"tool"} per ToolResult — NOT one batched user message.
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].role, "tool");
        assert_eq!(out[0].tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(out[0].content.as_deref(), Some("file contents"));
        assert_eq!(out[1].role, "tool");
        assert_eq!(out[1].tool_call_id.as_deref(), Some("call_2"));
    }

    #[test]
    fn translate_user_tool_result_with_error_prefixes_content() {
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: "ENOENT".into(),
                is_error: true,
            }],
        }];
        let out = to_chat_messages(&[], &messages).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].content.as_ref().unwrap().contains("[tool error]"));
        assert!(out[0].content.as_ref().unwrap().contains("ENOENT"));
    }

    #[test]
    fn strict_tools_serialize_with_strict_true() {
        let tools = vec![
            Tool {
                name: "read_file".into(),
                description: "read".into(),
                input_schema: json!({"type": "object"}),
                cache_control: None,
            },
            Tool {
                name: "submit_findings".into(),
                description: "submit".into(),
                input_schema: json!({"type": "object", "additionalProperties": false}),
                cache_control: None,
            },
        ];
        let out = to_chat_tools(&tools);
        assert_eq!(out[0].function.name, "read_file");
        assert!(
            out[0].function.strict.is_none(),
            "read tools should be non-strict"
        );
        assert_eq!(out[1].function.name, "submit_findings");
        assert_eq!(
            out[1].function.strict,
            Some(true),
            "submit_* tools must be strict"
        );
    }

    #[test]
    fn usage_subtracts_cached_tokens() {
        let u = ChatUsage {
            prompt_tokens: 1000,
            completion_tokens: 50,
            prompt_tokens_details: Some(PromptTokensDetails {
                cached_tokens: Some(700),
            }),
        };
        let mapped = to_usage(&u);
        assert_eq!(mapped.input_tokens, 300, "uncached portion");
        assert_eq!(mapped.cache_read_input_tokens, 700);
        assert_eq!(mapped.output_tokens, 50);
        assert_eq!(
            mapped.cache_creation_input_tokens, 0,
            "OpenAI never bills cache writes"
        );
    }

    #[test]
    fn usage_no_cached_tokens_field() {
        let u = ChatUsage {
            prompt_tokens: 500,
            completion_tokens: 20,
            prompt_tokens_details: None,
        };
        let mapped = to_usage(&u);
        assert_eq!(mapped.input_tokens, 500);
        assert_eq!(mapped.cache_read_input_tokens, 0);
    }

    // ---- response parsing ----

    #[test]
    fn from_chat_response_maps_tool_calls_to_tool_use_blocks() {
        let resp = ChatResponse {
            id: "chatcmpl_1".into(),
            model: "gpt-5-mini".into(),
            choices: vec![ChatChoice {
                message: ChatChoiceMessage {
                    content: None,
                    tool_calls: Some(vec![ToolCallResp {
                        id: "call_abc".into(),
                        function: FunctionCallResp {
                            name: "submit_findings".into(),
                            arguments: r#"{"findings":[]}"#.into(),
                        },
                    }]),
                },
                finish_reason: Some("tool_calls".into()),
            }],
            usage: ChatUsage::default(),
        };
        let out = from_chat_response(resp).unwrap();
        assert_eq!(out.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(out.content.len(), 1);
        match &out.content[0] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "call_abc");
                assert_eq!(name, "submit_findings");
                assert_eq!(input, &json!({"findings": []}));
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn from_chat_response_finish_reason_mapping() {
        for (raw, expected) in [
            ("stop", StopReason::EndTurn),
            ("length", StopReason::MaxTokens),
            ("tool_calls", StopReason::ToolUse),
        ] {
            let resp = ChatResponse {
                id: "id".into(),
                model: "gpt-5-mini".into(),
                choices: vec![ChatChoice {
                    message: ChatChoiceMessage {
                        content: Some("hi".into()),
                        tool_calls: None,
                    },
                    finish_reason: Some(raw.into()),
                }],
                usage: ChatUsage::default(),
            };
            let out = from_chat_response(resp).unwrap();
            assert_eq!(out.stop_reason, Some(expected), "raw={raw}");
        }
    }

    // ---- HTTP-level tests via wiremock ----

    fn sample_request() -> GenerationRequest {
        let mut req = GenerationRequest::new("gpt-5-mini", 1024);
        req.temperature = Some(0.0); // should be dropped on the wire
        req.system.push(
            SystemBlock::text("You are a security reviewer.")
                .with_cache(CacheControl::ephemeral_1h()),
        );
        req.messages.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "scan this file".into(),
            }],
        });
        req
    }

    #[tokio::test]
    async fn drops_temperature_for_gpt_5() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "chatcmpl_t",
                "object": "chat.completion",
                "model": "gpt-5-mini",
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": "ok" },
                    "finish_reason": "stop"
                }],
                "usage": { "prompt_tokens": 5, "completion_tokens": 1 }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let provider = OpenAiProvider::with_base_url("test-key", server.uri()).unwrap();
        provider.generate(sample_request()).await.unwrap();

        let received = &server.received_requests().await.unwrap()[0];
        let body: Value = serde_json::from_slice(&received.body).unwrap();
        assert_eq!(body["model"], "gpt-5-mini");
        assert_eq!(body["max_completion_tokens"], 1024);
        assert_eq!(body["stream"], false);
        assert!(
            body.get("temperature").is_none(),
            "gpt-5 must not see a temperature field on the wire (got {:?})",
            body.get("temperature")
        );
        assert_eq!(
            body["reasoning_effort"], "minimal",
            "gpt-5 must pin reasoning_effort=minimal so per-stage token budgets \
             aren't eaten by reasoning before the tool call lands"
        );
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["role"], "user");
    }

    #[tokio::test]
    async fn keeps_temperature_for_non_gpt_5_model() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "id", "model": "gpt-4.1",
                "choices": [{"index": 0, "message": {"role":"assistant","content":"ok"}, "finish_reason":"stop"}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1}
            })))
            .mount(&server)
            .await;
        let provider = OpenAiProvider::with_base_url("k", server.uri()).unwrap();
        let mut req = sample_request();
        req.model = "gpt-4.1".into();
        provider.generate(req).await.unwrap();
        let received = &server.received_requests().await.unwrap()[0];
        let body: Value = serde_json::from_slice(&received.body).unwrap();
        assert_eq!(body["temperature"], 0.0);
        assert!(
            body.get("reasoning_effort").is_none(),
            "non gpt-5 models must not see reasoning_effort on the wire"
        );
    }

    #[tokio::test]
    async fn truncated_length_response_is_surfaced_as_bad_request() {
        // gpt-5 returning finish_reason=length with empty content is the
        // classic "reasoning ate the budget" failure. The provider must NOT
        // silently return an empty Response — that hides the bug as
        // "model didn't call the tool" upstream.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "id",
                "model": "gpt-5-mini",
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": null },
                    "finish_reason": "length"
                }],
                "usage": {
                    "prompt_tokens": 100,
                    "completion_tokens": 256,
                    "completion_tokens_details": { "reasoning_tokens": 256 }
                }
            })))
            .mount(&server)
            .await;
        let provider = OpenAiProvider::with_base_url("k", server.uri()).unwrap();
        let err = provider.generate(sample_request()).await.unwrap_err();
        match err {
            ProviderError::BadRequest(msg) => {
                assert!(msg.contains("truncated"), "{msg}");
                assert!(msg.contains("reasoning"), "{msg}");
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn generate_maps_401_to_auth_failed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "error": { "type": "invalid_api_key", "message": "bad key" }
            })))
            .mount(&server)
            .await;
        let provider = OpenAiProvider::with_base_url("bad", server.uri()).unwrap();
        let err = provider.generate(sample_request()).await.unwrap_err();
        assert!(matches!(err, ProviderError::AuthFailed));
    }

    #[tokio::test]
    async fn generate_maps_429_with_retry_after() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("retry-after", "12")
                    .set_body_string("rate limited"),
            )
            .mount(&server)
            .await;
        let provider = OpenAiProvider::with_base_url("k", server.uri()).unwrap();
        let err = provider.generate(sample_request()).await.unwrap_err();
        match err {
            ProviderError::RateLimited { retry_after } => {
                assert_eq!(retry_after, Some(Duration::from_secs(12)));
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rate_limit_observer_records_under_openai_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-ratelimit-remaining-requests", "0")
                    .insert_header("x-ratelimit-reset-requests", "6s")
                    .insert_header("x-ratelimit-remaining-tokens", "12345")
                    .insert_header("x-ratelimit-reset-tokens", "30s")
                    .set_body_json(json!({
                        "id": "id", "model": "gpt-5-mini",
                        "choices": [{"index": 0, "message": {"role":"assistant","content":"ok"}, "finish_reason":"stop"}],
                        "usage": {"prompt_tokens": 1, "completion_tokens": 1}
                    })),
            )
            .mount(&server)
            .await;
        let observer = MultiObserver::new();
        let provider = OpenAiProvider::with_base_url("k", server.uri())
            .unwrap()
            .with_rate_limit_observer(observer.clone());
        provider.generate(sample_request()).await.unwrap();

        let snap = observer
            .current("openai")
            .expect("openai snapshot recorded");
        assert_eq!(snap.requests_remaining, Some(0));
        assert!(snap.requests_reset.is_some());
        assert_eq!(snap.input_tokens_remaining, Some(12345));
        assert!(observer.current("anthropic").is_none());
    }
}
