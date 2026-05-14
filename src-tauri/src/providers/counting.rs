//! A `Provider` wrapper that tallies `Usage` across every `generate()` call.
//!
//! Used by the orchestrator to attribute token spend per pipeline stage: a
//! single counter is shared by the wrapper, the orchestrator snapshots the
//! counter before each stage and subtracts to get the stage's contribution.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::stream::BoxStream;

use super::{GenerationRequest, Provider, Response, StreamEvent, Usage};
use crate::error::ProviderResult;

/// Atomic-ish usage accumulator. The single `Mutex` is fine — generate
/// calls take seconds, so contention is negligible compared to the call
/// itself.
#[derive(Debug, Default)]
pub struct UsageCounter {
    inner: Mutex<Usage>,
}

impl UsageCounter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn add(&self, u: &Usage) {
        let mut c = self.inner.lock().unwrap();
        c.input_tokens = c.input_tokens.saturating_add(u.input_tokens);
        c.output_tokens = c.output_tokens.saturating_add(u.output_tokens);
        c.cache_creation_input_tokens = c
            .cache_creation_input_tokens
            .saturating_add(u.cache_creation_input_tokens);
        c.cache_read_input_tokens = c
            .cache_read_input_tokens
            .saturating_add(u.cache_read_input_tokens);
    }

    pub fn snapshot(&self) -> Usage {
        self.inner.lock().unwrap().clone()
    }
}

/// `a - b` element-wise, saturating at zero. Used by the orchestrator to
/// compute per-stage usage from a shared counter.
pub fn diff(a: &Usage, b: &Usage) -> Usage {
    Usage {
        input_tokens: a.input_tokens.saturating_sub(b.input_tokens),
        output_tokens: a.output_tokens.saturating_sub(b.output_tokens),
        cache_creation_input_tokens: a
            .cache_creation_input_tokens
            .saturating_sub(b.cache_creation_input_tokens),
        cache_read_input_tokens: a
            .cache_read_input_tokens
            .saturating_sub(b.cache_read_input_tokens),
    }
}

/// Wraps an inner provider and adds the response's `Usage` to a shared
/// counter on every `generate()`. Streaming calls are forwarded verbatim
/// (no usage tracking on streams — we don't stream in the scan pipeline).
pub struct CountingProvider {
    inner: Arc<dyn Provider>,
    counter: Arc<UsageCounter>,
}

impl CountingProvider {
    pub fn new(inner: Arc<dyn Provider>, counter: Arc<UsageCounter>) -> Self {
        Self { inner, counter }
    }
}

#[async_trait]
impl Provider for CountingProvider {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    async fn generate(&self, req: GenerationRequest) -> ProviderResult<Response> {
        let resp = self.inner.generate(req).await?;
        self.counter.add(&resp.usage);
        Ok(resp)
    }

    async fn stream(
        &self,
        req: GenerationRequest,
    ) -> ProviderResult<BoxStream<'static, ProviderResult<StreamEvent>>> {
        self.inner.stream(req).await
    }
}

/// Wraps an inner provider and short-circuits every `generate()` /
/// `stream()` call with `ProviderError::Cancelled` once the shared flag
/// flips to true. Already-running HTTP requests aren't aborted — but every
/// agent loop checks `provider.generate()` before each iteration, so cancel
/// takes effect at the next round-trip without needing to thread a
/// cancellation token through every `*_many` signature.
pub struct CancellingProvider {
    inner: Arc<dyn Provider>,
    cancel: Arc<AtomicBool>,
}

impl CancellingProvider {
    pub fn new(inner: Arc<dyn Provider>, cancel: Arc<AtomicBool>) -> Self {
        Self { inner, cancel }
    }
}

#[async_trait]
impl Provider for CancellingProvider {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    async fn generate(&self, req: GenerationRequest) -> ProviderResult<Response> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(crate::error::ProviderError::Cancelled);
        }
        self.inner.generate(req).await
    }

    async fn stream(
        &self,
        req: GenerationRequest,
    ) -> ProviderResult<BoxStream<'static, ProviderResult<StreamEvent>>> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(crate::error::ProviderError::Cancelled);
        }
        self.inner.stream(req).await
    }
}
