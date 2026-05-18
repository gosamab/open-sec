//! `Provider` decorators layered on top of the inner Anthropic client:
//! token counting, cancellation, and rate-limit retry. Each wraps an
//! `Arc<dyn Provider>` and forwards `generate` with a small slice of
//! extra behaviour.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tracing::{info, warn};

use super::{GenerationRequest, Provider, Response, Usage};
use crate::error::{ProviderError, ProviderResult};

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
/// counter on every `generate()`.
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
}

/// Wraps an inner provider and short-circuits every `generate()` call with
/// `ProviderError::Cancelled` once the shared flag flips to true.
/// Already-running HTTP requests aren't aborted — but every agent loop
/// checks `provider.generate()` before each iteration, so cancel takes
/// effect at the next round-trip without needing to thread a cancellation
/// token through every `*_many` signature.
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
}

/// Notification fired when the retry wrapper is about to sleep, so the
/// orchestrator can surface "rate-limited, retrying in Xs" through the UI
/// event stream. `attempt` is 1-indexed (first retry = 1).
pub type RetryNotify = Arc<dyn Fn(Duration, u32) + Send + Sync>;

/// Auto-retry on `ProviderError::RateLimited`. Sleeps for `retry_after` (or
/// `default_backoff` when the API didn't give us one), capped at `max_sleep`,
/// up to `max_attempts` times. Cancellation is honored both at the call
/// boundary and during the sleep itself (we poll every 250ms instead of
/// `tokio::time::sleep` so a cancel-flip wakes us up promptly).
pub struct RetryingProvider {
    inner: Arc<dyn Provider>,
    cancel: Option<Arc<AtomicBool>>,
    notify: Option<RetryNotify>,
    max_attempts: u32,
    default_backoff: Duration,
    max_sleep: Duration,
}

impl RetryingProvider {
    pub fn new(inner: Arc<dyn Provider>) -> Self {
        Self {
            inner,
            cancel: None,
            notify: None,
            max_attempts: 5,
            default_backoff: Duration::from_secs(10),
            max_sleep: Duration::from_secs(60),
        }
    }

    pub fn with_cancel(mut self, flag: Arc<AtomicBool>) -> Self {
        self.cancel = Some(flag);
        self
    }

    pub fn with_notify(mut self, notify: RetryNotify) -> Self {
        self.notify = Some(notify);
        self
    }

    fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|f| f.load(Ordering::Relaxed))
    }

    /// Cancellable sleep. Polls the cancel flag every 250ms so the user
    /// doesn't have to wait the full retry-after for cancel to take effect.
    async fn sleep_or_cancel(&self, dur: Duration) -> ProviderResult<()> {
        let tick = Duration::from_millis(250);
        let mut remaining = dur;
        while !remaining.is_zero() {
            if self.is_cancelled() {
                return Err(ProviderError::Cancelled);
            }
            let chunk = remaining.min(tick);
            tokio::time::sleep(chunk).await;
            remaining = remaining.saturating_sub(chunk);
        }
        Ok(())
    }

    fn sleep_for(&self, retry_after: Option<Duration>) -> Duration {
        retry_after.unwrap_or(self.default_backoff).min(self.max_sleep)
    }
}

#[async_trait]
impl Provider for RetryingProvider {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    async fn generate(&self, req: GenerationRequest) -> ProviderResult<Response> {
        for attempt in 0..self.max_attempts {
            match self.inner.generate(req.clone()).await {
                Err(ProviderError::RateLimited { retry_after }) => {
                    if attempt + 1 >= self.max_attempts {
                        warn!(attempt = attempt + 1, "rate-limited and out of retries");
                        return Err(ProviderError::RateLimited { retry_after });
                    }
                    let sleep = self.sleep_for(retry_after);
                    info!(
                        attempt = attempt + 1,
                        sleep_secs = sleep.as_secs(),
                        "rate-limited; sleeping and retrying"
                    );
                    if let Some(notify) = &self.notify {
                        notify(sleep, attempt + 1);
                    }
                    self.sleep_or_cancel(sleep).await?;
                    continue;
                }
                other => return other,
            }
        }
        Err(ProviderError::RateLimited { retry_after: None })
    }
}
