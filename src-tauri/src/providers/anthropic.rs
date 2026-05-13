use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use reqwest::{header, Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, instrument, warn};

use crate::error::{ProviderError, ProviderResult};
use crate::providers::{
    ContentBlock, GenerationRequest, Message, Provider, Response, StopReason, StreamEvent, Usage,
};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const EXTENDED_CACHE_BETA: &str = "extended-cache-ttl-2025-04-11";

pub struct AnthropicProvider {
    api_key: String,
    base_url: String,
    client: Client,
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
        })
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

    #[instrument(skip(self, req), fields(provider = "anthropic", model = %req.model, stream = false))]
    async fn generate(&self, req: GenerationRequest) -> ProviderResult<Response> {
        let body = AnthropicRequestBody {
            inner: &req,
            stream: false,
        };

        let resp = self.build_request(&body).send().await?;
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

    #[instrument(skip(self, req), fields(provider = "anthropic", model = %req.model, stream = true))]
    async fn stream(
        &self,
        req: GenerationRequest,
    ) -> ProviderResult<BoxStream<'static, ProviderResult<StreamEvent>>> {
        let body = AnthropicRequestBody {
            inner: &req,
            stream: true,
        };

        let resp = self.build_request(&body).send().await?;
        if !resp.status().is_success() {
            return Err(classify_error(resp).await);
        }

        let bytes_stream = resp.bytes_stream();
        let (tx, rx) = mpsc::channel::<ProviderResult<StreamEvent>>(64);

        tokio::spawn(sse_loop(bytes_stream, tx));

        let stream = futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        });
        Ok(Box::pin(stream))
    }
}

// --- SSE parsing ---------------------------------------------------------

async fn sse_loop<S>(mut bytes_stream: S, tx: mpsc::Sender<ProviderResult<StreamEvent>>)
where
    S: futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin + Send,
{
    let mut buffer: Vec<u8> = Vec::with_capacity(4096);

    while let Some(chunk) = bytes_stream.next().await {
        let chunk = match chunk {
            Ok(b) => b,
            Err(e) => {
                let _ = tx.send(Err(ProviderError::Network(e))).await;
                return;
            }
        };
        buffer.extend_from_slice(&chunk);

        while let Some((event_bytes, consume)) = take_event(&buffer) {
            match parse_anthropic_event(&event_bytes) {
                Ok(Some(evt)) => {
                    if tx.send(Ok(evt)).await.is_err() {
                        return;
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            }
            buffer.drain(..consume);
        }
    }
}

/// Locate the first complete SSE event in `buf` and return its body + the number
/// of bytes (including the trailing blank-line separator) to drain from the buffer.
fn take_event(buf: &[u8]) -> Option<(Vec<u8>, usize)> {
    let (boundary, sep_len) = find_event_boundary(buf)?;
    Some((buf[..boundary].to_vec(), boundary + sep_len))
}

fn find_event_boundary(buf: &[u8]) -> Option<(usize, usize)> {
    // SSE separates events with a blank line: "\n\n" or "\r\n\r\n".
    let crlf = b"\r\n\r\n";
    let lf = b"\n\n";

    let crlf_idx = window_find(buf, crlf);
    let lf_idx = window_find(buf, lf);

    match (crlf_idx, lf_idx) {
        (Some(a), Some(b)) if a < b => Some((a, crlf.len())),
        (Some(_), Some(b)) => Some((b, lf.len())),
        (Some(a), None) => Some((a, crlf.len())),
        (None, Some(b)) => Some((b, lf.len())),
        (None, None) => None,
    }
}

fn window_find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

fn parse_anthropic_event(raw: &[u8]) -> ProviderResult<Option<StreamEvent>> {
    // Each event is one or more lines. We care about "data:" lines (concatenated
    // with \n if multiple). The "event:" header is informational; the JSON payload
    // already carries a `type` field that we dispatch on.
    let text = std::str::from_utf8(raw).map_err(|e| ProviderError::Stream(e.to_string()))?;
    let mut data = String::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        }
        // ignore "event:" / "id:" / "retry:" — JSON payload has the event type
    }

    if data.is_empty() {
        return Ok(None);
    }

    let event: AnthropicSseEvent = serde_json::from_str(&data)?;
    Ok(convert_event(event))
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicSseEvent {
    MessageStart {
        message: SseMessageStart,
    },
    ContentBlockStart {
        index: u32,
        content_block: ContentBlock,
    },
    ContentBlockDelta {
        index: u32,
        delta: BlockDelta,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: MessageDeltaInner,
        #[serde(default)]
        usage: Usage,
    },
    MessageStop,
    Ping,
    Error {
        error: ApiErrorInner,
    },
}

#[derive(Deserialize)]
struct SseMessageStart {
    id: String,
    model: String,
    #[serde(default)]
    usage: Usage,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BlockDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
}

#[derive(Deserialize)]
struct MessageDeltaInner {
    #[serde(default)]
    stop_reason: Option<StopReason>,
    #[serde(default)]
    stop_sequence: Option<String>,
}

fn convert_event(evt: AnthropicSseEvent) -> Option<StreamEvent> {
    Some(match evt {
        AnthropicSseEvent::MessageStart { message } => StreamEvent::MessageStart {
            id: message.id,
            model: message.model,
            usage: message.usage,
        },
        AnthropicSseEvent::ContentBlockStart {
            index,
            content_block,
        } => StreamEvent::ContentBlockStart {
            index,
            block: content_block,
        },
        AnthropicSseEvent::ContentBlockDelta {
            index,
            delta: BlockDelta::TextDelta { text },
        } => StreamEvent::TextDelta { index, text },
        AnthropicSseEvent::ContentBlockDelta {
            index,
            delta: BlockDelta::InputJsonDelta { partial_json },
        } => StreamEvent::InputJsonDelta {
            index,
            partial_json,
        },
        AnthropicSseEvent::ContentBlockStop { index } => StreamEvent::ContentBlockStop { index },
        AnthropicSseEvent::MessageDelta { delta, usage } => StreamEvent::MessageDelta {
            stop_reason: delta.stop_reason,
            stop_sequence: delta.stop_sequence,
            usage,
        },
        AnthropicSseEvent::MessageStop => StreamEvent::MessageStop,
        AnthropicSseEvent::Ping => return None,
        AnthropicSseEvent::Error { error } => {
            warn!(kind = %error.kind, message = %error.message, "anthropic stream error event");
            return None;
        }
    })
}

// Re-export the inner message types as private helpers so we don't leak them.
// The `Message` type from the parent module is intentionally untouched here.
#[allow(dead_code)]
fn _assert_message_type(m: Message) -> Message {
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_message_start() {
        let raw = b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude-sonnet-4-6\",\"role\":\"assistant\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}";
        let evt = parse_anthropic_event(raw).unwrap().unwrap();
        match evt {
            StreamEvent::MessageStart { id, model, usage } => {
                assert_eq!(id, "msg_1");
                assert_eq!(model, "claude-sonnet-4-6");
                assert_eq!(usage.input_tokens, 10);
                assert_eq!(usage.output_tokens, 1);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parse_text_delta() {
        let raw = b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}";
        let evt = parse_anthropic_event(raw).unwrap().unwrap();
        match evt {
            StreamEvent::TextDelta { index, text } => {
                assert_eq!(index, 0);
                assert_eq!(text, "Hello");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parse_tool_use_input_json_delta() {
        let raw = b"data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}";
        let evt = parse_anthropic_event(raw).unwrap().unwrap();
        match evt {
            StreamEvent::InputJsonDelta {
                index,
                partial_json,
            } => {
                assert_eq!(index, 1);
                assert_eq!(partial_json, "{\"path\":");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parse_message_delta_with_usage() {
        let raw = b"data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":42}}";
        let evt = parse_anthropic_event(raw).unwrap().unwrap();
        match evt {
            StreamEvent::MessageDelta {
                stop_reason,
                usage,
                ..
            } => {
                assert_eq!(stop_reason, Some(StopReason::EndTurn));
                assert_eq!(usage.output_tokens, 42);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn ping_is_filtered() {
        let raw = b"data: {\"type\":\"ping\"}";
        assert!(parse_anthropic_event(raw).unwrap().is_none());
    }

    #[test]
    fn event_boundary_split() {
        let buf = b"data: {\"type\":\"ping\"}\n\ndata: {\"type\":\"message_stop\"}\n\n";
        let (first, consumed) = take_event(buf).unwrap();
        assert_eq!(consumed, b"data: {\"type\":\"ping\"}\n\n".len());
        assert!(first.starts_with(b"data:"));
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use crate::providers::{CacheControl, Role, SystemBlock};
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

        // Inspect the recorded request body shape.
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

    #[tokio::test]
    async fn stream_parses_sse_chunks() {
        let server = MockServer::start().await;

        let sse_body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_s\",\"model\":\"claude-sonnet-4-6\",\"role\":\"assistant\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":5,\"output_tokens\":1}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":1}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse_body),
            )
            .mount(&server)
            .await;

        let provider = AnthropicProvider::with_base_url("k", server.uri()).unwrap();
        let mut stream = provider.stream(sample_request()).await.unwrap();

        let mut events = Vec::new();
        while let Some(evt) = stream.next().await {
            events.push(evt.unwrap());
        }

        // Expect: message_start, content_block_start, text_delta, content_block_stop,
        // message_delta, message_stop  (6 events; ping would be filtered)
        assert_eq!(events.len(), 6, "got events: {events:#?}");
        assert!(matches!(events[0], StreamEvent::MessageStart { .. }));
        assert!(matches!(events[1], StreamEvent::ContentBlockStart { .. }));
        match &events[2] {
            StreamEvent::TextDelta { text, .. } => assert_eq!(text, "hi"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
        assert!(matches!(events[3], StreamEvent::ContentBlockStop { .. }));
        assert!(matches!(
            events[4],
            StreamEvent::MessageDelta {
                stop_reason: Some(StopReason::EndTurn),
                ..
            }
        ));
        assert!(matches!(events[5], StreamEvent::MessageStop));

        // Verify the stream request body had stream: true.
        let received = &server.received_requests().await.unwrap()[0];
        let body: Value = serde_json::from_slice(&received.body).unwrap();
        assert_eq!(body["stream"], true);
    }
}
