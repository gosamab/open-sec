pub mod anthropic;
pub mod counting;
pub mod multiplex;
pub mod openai;
pub mod rate_limit;

/// Map a model id to the stable key of the provider that handles it.
/// `claude-*` → `"anthropic"`, `gpt-*` → `"openai"`. Anything else falls
/// back to `"anthropic"` so an unknown model still surfaces as an Anthropic
/// `BadRequest` rather than a routing panic. The multiplex provider is the
/// authoritative gate — see [`crate::providers::multiplex`].
pub fn route_model_to_provider(model: &str) -> &'static str {
    if model.starts_with("gpt-") {
        "openai"
    } else {
        "anthropic"
    }
}

use async_trait::async_trait;
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

#[async_trait]
pub trait Provider: Send + Sync {
    /// Stable name used in logs / config (e.g. "anthropic").
    fn name(&self) -> &'static str;

    /// Run a non-streaming generation. Returns the full assistant message.
    async fn generate(&self, req: GenerationRequest) -> ProviderResult<Response>;
}

#[cfg(test)]
pub mod test_support {
    use serde_json::Value;

    /// Assert a JSON Schema satisfies OpenAI Chat Completions strict-mode
    /// tool rules: every object schema must set `additionalProperties: false`
    /// and list every property in `required`. Recurses into `properties.*`
    /// and `items`. Panics on the first violation with a JSON-path-style
    /// pointer to the offending node.
    pub fn assert_openai_strict_compatible(schema: &Value) {
        walk(schema, "$");
    }

    fn walk(node: &Value, path: &str) {
        let Some(obj) = node.as_object() else { return };
        if is_object_schema(obj.get("type")) {
            let ap = obj.get("additionalProperties");
            assert!(
                matches!(ap, Some(Value::Bool(false))),
                "object at {path} must set additionalProperties: false (got {ap:?})"
            );
            if let Some(props) = obj.get("properties").and_then(|p| p.as_object()) {
                let required: Vec<&str> = obj
                    .get("required")
                    .and_then(|r| r.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();
                for key in props.keys() {
                    assert!(
                        required.iter().any(|r| r == key),
                        "object at {path} omits '{key}' from required (strict mode requires every property)"
                    );
                }
                for (key, sub) in props {
                    walk(sub, &format!("{path}.{key}"));
                }
            }
        }
        if let Some(items) = obj.get("items") {
            walk(items, &format!("{path}[]"));
        }
    }

    fn is_object_schema(t: Option<&Value>) -> bool {
        match t {
            Some(Value::String(s)) => s == "object",
            Some(Value::Array(a)) => a.iter().any(|v| v.as_str() == Some("object")),
            _ => false,
        }
    }
}
