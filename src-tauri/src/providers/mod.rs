#![allow(dead_code)] // wired up incrementally as Step 3+ land

pub mod anthropic;
pub mod counting;

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ProviderResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "is_false")]
        is_error: bool,
    },
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheTtl {
    #[serde(rename = "5m")]
    FiveMinutes,
    #[serde(rename = "1h")]
    OneHour,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CacheControl {
    Ephemeral {
        #[serde(skip_serializing_if = "Option::is_none")]
        ttl: Option<CacheTtl>,
    },
}

impl CacheControl {
    pub fn ephemeral_1h() -> Self {
        Self::Ephemeral {
            ttl: Some(CacheTtl::OneHour),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemBlock {
    #[serde(rename = "type", default = "system_block_kind")]
    pub kind: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

fn system_block_kind() -> String {
    "text".to_string()
}

impl SystemBlock {
    pub fn text<S: Into<String>>(text: S) -> Self {
        Self {
            kind: system_block_kind(),
            text: text.into(),
            cache_control: None,
        }
    }

    pub fn with_cache(mut self, cache: CacheControl) -> Self {
        self.cache_control = Some(cache);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub system: Vec<SystemBlock>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
}

impl GenerationRequest {
    pub fn new(model: impl Into<String>, max_tokens: u32) -> Self {
        Self {
            model: model.into(),
            max_tokens,
            messages: Vec::new(),
            system: Vec::new(),
            tools: Vec::new(),
            temperature: None,
            top_p: None,
            stop_sequences: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
    PauseTurn,
    Refusal,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: String,
    pub model: String,
    pub content: Vec<ContentBlock>,
    pub stop_reason: Option<StopReason>,
    pub stop_sequence: Option<String>,
    pub usage: Usage,
}

/// Streaming events emitted by `Provider::stream`.
///
/// Mirrors Anthropic's SSE event shape. The detection agent only needs to react
/// to the high-level lifecycle (start/text/tool-input/stop) and the final usage
/// delta, so we expose those without forcing callers to handle every raw frame.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// First event of the stream; carries the assistant message id/model/initial usage.
    MessageStart { id: String, model: String, usage: Usage },
    /// A new content block began at `index`. The block's full shape is known once
    /// `ContentBlockStop` arrives; for `tool_use`, intermediate `input_json` deltas
    /// stream the JSON payload piecewise.
    ContentBlockStart { index: u32, block: ContentBlock },
    /// Incremental text appended to the text block at `index`.
    TextDelta { index: u32, text: String },
    /// Incremental JSON fragment appended to a tool_use block at `index`.
    InputJsonDelta { index: u32, partial_json: String },
    /// Content block at `index` finished.
    ContentBlockStop { index: u32 },
    /// Final message-level delta: stop reason + usage update.
    MessageDelta {
        stop_reason: Option<StopReason>,
        stop_sequence: Option<String>,
        usage: Usage,
    },
    /// Stream terminator.
    MessageStop,
}

#[async_trait]
pub trait Provider: Send + Sync {
    /// Stable name used in logs / config (e.g. "anthropic", "llama-cpp").
    fn name(&self) -> &'static str;

    /// Run a non-streaming generation. Returns the full assistant message.
    async fn generate(&self, req: GenerationRequest) -> ProviderResult<Response>;

    /// Run a streaming generation. Each yielded item is one parsed `StreamEvent`
    /// or a `ProviderError` if the stream broke partway through.
    async fn stream(
        &self,
        req: GenerationRequest,
    ) -> ProviderResult<BoxStream<'static, ProviderResult<StreamEvent>>>;
}
