//! Per-stage provider routing. Dispatches `generate()` to one of the
//! configured inner providers based on the model id prefix
//! (see [`super::route_model_to_provider`]). Lets the pipeline mix Anthropic
//! and OpenAI stages while sharing one decorator stack (Cancelling →
//! Counting → RateLimited → Retrying → Multiplex → {anthropic, openai}).
//!
//! If a stage's model routes to a provider that wasn't configured (no API
//! key), `generate` returns `BadRequest`. The orchestrator's fail-fast gate
//! is supposed to catch this earlier, but the multiplex's own check is the
//! authoritative backstop.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::{ProviderError, ProviderResult};
use crate::providers::{route_model_to_provider, GenerationRequest, Provider, Response};

pub struct MultiplexProvider {
    anthropic: Option<Arc<dyn Provider>>,
    openai: Option<Arc<dyn Provider>>,
}

impl MultiplexProvider {
    pub fn new() -> Self {
        Self {
            anthropic: None,
            openai: None,
        }
    }

    pub fn with_anthropic(mut self, provider: Arc<dyn Provider>) -> Self {
        self.anthropic = Some(provider);
        self
    }

    pub fn with_openai(mut self, provider: Arc<dyn Provider>) -> Self {
        self.openai = Some(provider);
        self
    }
}

impl Default for MultiplexProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for MultiplexProvider {
    fn name(&self) -> &'static str {
        "multiplex"
    }

    async fn generate(&self, req: GenerationRequest) -> ProviderResult<Response> {
        let key = route_model_to_provider(&req.model);
        let routed = match key {
            "anthropic" => self.anthropic.as_ref(),
            "openai" => self.openai.as_ref(),
            _ => None,
        };
        let routed = routed.ok_or_else(|| {
            ProviderError::BadRequest(format!(
                "model '{}' routes to provider '{}', but that provider is not configured",
                req.model, key
            ))
        })?;
        routed.generate(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{Response, StopReason, Usage};
    use std::sync::Mutex;

    struct FakeProvider {
        name: &'static str,
        seen: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl Provider for FakeProvider {
        fn name(&self) -> &'static str {
            self.name
        }
        async fn generate(&self, req: GenerationRequest) -> ProviderResult<Response> {
            self.seen.lock().unwrap().push(req.model.clone());
            Ok(Response {
                id: "fake".into(),
                model: req.model,
                content: Vec::new(),
                stop_reason: Some(StopReason::EndTurn),
                stop_sequence: None,
                usage: Usage::default(),
            })
        }
    }

    fn fake(name: &'static str) -> Arc<FakeProvider> {
        Arc::new(FakeProvider {
            name,
            seen: Mutex::new(Vec::new()),
        })
    }

    #[tokio::test]
    async fn routes_by_model_prefix() {
        let anth = fake("anthropic-fake");
        let oai = fake("openai-fake");
        let mux = MultiplexProvider::new()
            .with_anthropic(Arc::clone(&anth) as Arc<dyn Provider>)
            .with_openai(Arc::clone(&oai) as Arc<dyn Provider>);

        mux.generate(GenerationRequest::new("claude-haiku-4-5", 100))
            .await
            .unwrap();
        mux.generate(GenerationRequest::new("gpt-5-mini", 100))
            .await
            .unwrap();

        assert_eq!(
            anth.seen.lock().unwrap().as_slice(),
            &["claude-haiku-4-5".to_string()]
        );
        assert_eq!(
            oai.seen.lock().unwrap().as_slice(),
            &["gpt-5-mini".to_string()]
        );
    }

    #[tokio::test]
    async fn errors_when_routed_provider_missing() {
        let anth = fake("anthropic-fake");
        let mux =
            MultiplexProvider::new().with_anthropic(Arc::clone(&anth) as Arc<dyn Provider>);

        let err = mux
            .generate(GenerationRequest::new("gpt-5-mini", 100))
            .await
            .unwrap_err();
        match err {
            ProviderError::BadRequest(msg) => {
                assert!(msg.contains("gpt-5-mini"), "{msg}");
                assert!(msg.contains("openai"), "{msg}");
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_model_falls_back_to_anthropic_routing() {
        let anth = fake("anthropic-fake");
        let mux =
            MultiplexProvider::new().with_anthropic(Arc::clone(&anth) as Arc<dyn Provider>);

        // route_model_to_provider returns "anthropic" for unknown prefixes,
        // so an unrecognized model routes to anthropic and the inner provider
        // handles validation.
        mux.generate(GenerationRequest::new("mistral-large", 100))
            .await
            .unwrap();
        assert_eq!(
            anth.seen.lock().unwrap().as_slice(),
            &["mistral-large".to_string()]
        );
    }
}
