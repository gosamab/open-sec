use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::{header, Client, StatusCode};
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use crate::error::{ProviderError, ProviderResult};
use crate::providers::rate_limit::{RateLimitObserver, RateLimitSnapshot};
use crate::providers::{
    ContentBlock, GenerationRequest, Provider, Response, StopReason, Usage,
};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const EXTENDED_CACHE_BETA: &str = "extended-cache-ttl-2025-04-11";

pub struct AnthropicProvider {
    api_key: String,
    base_url: String,
    client: Client,
    /// Where to publish parsed `anthropic-ratelimit-*` headers after each
    /// response. `RateLimitedProvider` reads this to pace upstream calls.
    observer: Option<Arc<RateLimitObserver>>,
}

impl AnthropicProvider {
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

    pub fn with_rate_limit_observer(mut self, observer: Arc<RateLimitObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    fn messages_url(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }

    fn build_request(&self, body: &AnthropicRequestBody<'_>) -> reqwest::RequestBuilder {
        self.client
            .post(self.messages_url())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("anthropic-beta", EXTENDED_CACHE_BETA)
            .header(header::CONTENT_TYPE, "application/json")
            .json(body)
    }
}

#[derive(Serialize)]
struct AnthropicRequestBody<'a> {
    #[serde(flatten)]
    inner: &'a GenerationRequest,
    stream: bool,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    id: String,
    model: String,
    content: Vec<ContentBlock>,
    stop_reason: Option<StopReason>,
    #[serde(default)]
    stop_sequence: Option<String>,
    #[serde(default)]
    usage: Usage,
}

impl From<AnthropicResponse> for Response {
    fn from(r: AnthropicResponse) -> Self {
        Response {
            id: r.id,
            model: r.model,
            content: r.content,
            stop_reason: r.stop_reason,
            stop_sequence: r.stop_sequence,
            usage: r.usage,
        }
    }
}

#[derive(Deserialize)]
struct ApiErrorBody {
    error: ApiErrorInner,
}

#[derive(Deserialize)]
struct ApiErrorInner {
    #[serde(rename = "type")]
    kind: String,
    message: String,
}

async fn classify_error(resp: reqwest::Response) -> ProviderError {
    let status = resp.status();
    let retry_after = parse_retry_after(&resp);
    let body = resp.text().await.unwrap_or_default();

    let parsed_message = serde_json::from_str::<ApiErrorBody>(&body)
        .ok()
        .map(|e| format!("{}: {}", e.error.kind, e.error.message));
    let message = parsed_message.unwrap_or(body.clone());

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

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    #[instrument(skip(self, req), fields(provider = "anthropic", model = %req.model))]
    async fn generate(&self, req: GenerationRequest) -> ProviderResult<Response> {
        let body = AnthropicRequestBody {
            inner: &req,
            stream: false,
        };

        let resp = self.build_request(&body).send().await?;
        if let Some(observer) = &self.observer {
            observer.record(RateLimitSnapshot::from_headers(resp.headers()));
        }
        if !resp.status().is_success() {
            return Err(classify_error(resp).await);
        }
        let parsed: AnthropicResponse = resp.json().await?;
        debug!(
            input_tokens = parsed.usage.input_tokens,
            output_tokens = parsed.usage.output_tokens,
            cache_read = parsed.usage.cache_read_input_tokens,
            cache_create = parsed.usage.cache_creation_input_tokens,
            "anthropic generate finished"
        );
        Ok(parsed.into())
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use crate::providers::{CacheControl, ContentBlock, Message, Role, SystemBlock};
    use serde_json::{json, Value};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn sample_request() -> GenerationRequest {
        let mut req = GenerationRequest::new("claude-sonnet-4-6", 1024);
        req.system.push(
            SystemBlock::text("You are a careful security reviewer.")
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
    async fn generate_sends_expected_headers_and_body() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "test-key"))
            .and(header("anthropic-version", ANTHROPIC_VERSION))
            .and(header("anthropic-beta", EXTENDED_CACHE_BETA))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "msg_test",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4-6",
                "content": [{ "type": "text", "text": "ok" }],
                "stop_reason": "end_turn",
                "stop_sequence": null,
                "usage": {
                    "input_tokens": 12,
                    "output_tokens": 3,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": 0
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let provider = AnthropicProvider::with_base_url("test-key", server.uri()).unwrap();
        let resp = provider.generate(sample_request()).await.unwrap();

        assert_eq!(resp.id, "msg_test");
        assert_eq!(resp.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(resp.usage.input_tokens, 12);

        let received = &server.received_requests().await.unwrap()[0];
        let body: Value = serde_json::from_slice(&received.body).unwrap();
        assert_eq!(body["model"], "claude-sonnet-4-6");
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(body["stream"], false);
        assert_eq!(body["system"][0]["type"], "text");
        assert_eq!(
            body["system"][0]["cache_control"]["type"],
            "ephemeral",
            "system block should carry cache_control"
        );
        assert_eq!(body["system"][0]["cache_control"]["ttl"], "1h");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"][0]["type"], "text");
    }

    #[tokio::test]
    async fn generate_maps_401_to_auth_failed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "type": "error",
                "error": { "type": "authentication_error", "message": "invalid key" }
            })))
            .mount(&server)
            .await;

        let provider = AnthropicProvider::with_base_url("bad", server.uri()).unwrap();
        let err = provider.generate(sample_request()).await.unwrap_err();
        assert!(matches!(err, ProviderError::AuthFailed));
    }

    #[tokio::test]
    async fn generate_maps_429_with_retry_after() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("retry-after", "7")
                    .set_body_string("rate limited"),
            )
            .mount(&server)
            .await;

        let provider = AnthropicProvider::with_base_url("k", server.uri()).unwrap();
        let err = provider.generate(sample_request()).await.unwrap_err();
        match err {
            ProviderError::RateLimited { retry_after } => {
                assert_eq!(retry_after, Some(Duration::from_secs(7)));
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }
}
