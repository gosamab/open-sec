//! Proactive rate-limit pacing for the Anthropic API.
//!
//! `AnthropicProvider` writes the latest `anthropic-ratelimit-*` headers into
//! a shared [`RateLimitObserver`] after every response. [`RateLimitedProvider`]
//! reads that observer before each call and sleeps until reset when a counter
//! has hit zero — turning a soon-to-be-429 into a clean wait, instead of the
//! cascading retry storm that happens when many concurrent calls all 429 at
//! once. Stack position is documented in CLAUDE.md.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use reqwest::header::HeaderMap;
use tracing::{info, warn};

use super::{GenerationRequest, Provider, Response};
use crate::error::ProviderResult;

/// Latest parsed `anthropic-ratelimit-*` headers. All fields are optional —
/// Anthropic doesn't always populate every counter, and we tolerate that.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RateLimitSnapshot {
    pub requests_remaining: Option<u64>,
    pub requests_reset: Option<SystemTime>,
    pub input_tokens_remaining: Option<u64>,
    pub input_tokens_reset: Option<SystemTime>,
    pub output_tokens_remaining: Option<u64>,
    pub output_tokens_reset: Option<SystemTime>,
}

impl RateLimitSnapshot {
    /// Read the `anthropic-ratelimit-*` headers from a response. Doesn't
    /// consume the response body.
    pub fn from_headers(h: &HeaderMap) -> Self {
        Self {
            requests_remaining: parse_u64(h, "anthropic-ratelimit-requests-remaining"),
            requests_reset: parse_reset(h, "anthropic-ratelimit-requests-reset"),
            input_tokens_remaining: parse_u64(h, "anthropic-ratelimit-input-tokens-remaining"),
            input_tokens_reset: parse_reset(h, "anthropic-ratelimit-input-tokens-reset"),
            output_tokens_remaining: parse_u64(h, "anthropic-ratelimit-output-tokens-remaining"),
            output_tokens_reset: parse_reset(h, "anthropic-ratelimit-output-tokens-reset"),
        }
    }

    /// How long to wait before the next call. `None` means no exhausted
    /// counter — caller can proceed immediately. Otherwise returns the
    /// longest reset wait among exhausted counters plus `safety`.
    ///
    /// "Exhausted" means `remaining == 0`. A small safety margin avoids
    /// firing the moment the bucket nominally refills (clock skew).
    pub fn wait_until_reset(&self, now: SystemTime, safety: Duration) -> Option<Duration> {
        let mut max_wait: Option<Duration> = None;
        let pairs = [
            (self.requests_remaining, self.requests_reset),
            (self.input_tokens_remaining, self.input_tokens_reset),
            (self.output_tokens_remaining, self.output_tokens_reset),
        ];
        for (rem, reset_at) in pairs {
            if let (Some(0), Some(reset_at)) = (rem, reset_at) {
                let dur = reset_at.duration_since(now).unwrap_or(Duration::ZERO) + safety;
                max_wait = Some(max_wait.map_or(dur, |m| m.max(dur)));
            }
        }
        max_wait
    }
}

/// Shared mutable cell holding the latest snapshot. Writer:
/// `AnthropicProvider`. Reader: `RateLimitedProvider`.
#[derive(Debug, Default)]
pub struct RateLimitObserver {
    snapshot: Mutex<Option<RateLimitSnapshot>>,
}

impl RateLimitObserver {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn record(&self, snapshot: RateLimitSnapshot) {
        *self.snapshot.lock().unwrap() = Some(snapshot);
    }

    pub fn current(&self) -> Option<RateLimitSnapshot> {
        self.snapshot.lock().unwrap().clone()
    }
}

/// `Provider` decorator that sleeps before each call when the most recent
/// snapshot shows an exhausted counter. Sleeps cap at `max_wait` to bound
/// pathological clock skew or stale snapshots.
pub struct RateLimitedProvider {
    inner: Arc<dyn Provider>,
    observer: Arc<RateLimitObserver>,
    safety: Duration,
    max_wait: Duration,
}

impl RateLimitedProvider {
    pub fn new(inner: Arc<dyn Provider>, observer: Arc<RateLimitObserver>) -> Self {
        Self {
            inner,
            observer,
            safety: Duration::from_millis(500),
            max_wait: Duration::from_secs(120),
        }
    }
}

#[async_trait]
impl Provider for RateLimitedProvider {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    async fn generate(&self, req: GenerationRequest) -> ProviderResult<Response> {
        if let Some(snap) = self.observer.current() {
            if let Some(wait) = snap.wait_until_reset(SystemTime::now(), self.safety) {
                let capped = wait.min(self.max_wait);
                if capped >= Duration::from_millis(50) {
                    info!(
                        wait_ms = capped.as_millis() as u64,
                        "anthropic rate-limit exhausted; pacing before next call"
                    );
                    tokio::time::sleep(capped).await;
                }
                if wait > self.max_wait {
                    warn!(
                        wait_ms = wait.as_millis() as u64,
                        cap_ms = self.max_wait.as_millis() as u64,
                        "rate-limit reset is further than cap; will retry early and possibly 429"
                    );
                }
            }
        }
        self.inner.generate(req).await
    }
}

fn parse_u64(h: &HeaderMap, name: &str) -> Option<u64> {
    h.get(name)?.to_str().ok()?.parse().ok()
}

fn parse_reset(h: &HeaderMap, name: &str) -> Option<SystemTime> {
    parse_rfc3339_utc(h.get(name)?.to_str().ok()?)
}

/// Minimal RFC 3339 parser for the format Anthropic actually emits:
/// `YYYY-MM-DDTHH:MM:SSZ` (optionally with fractional seconds, which we
/// truncate). Avoids pulling in `chrono`/`time` just for this one parser.
fn parse_rfc3339_utc(s: &str) -> Option<SystemTime> {
    let s = s.trim();
    let s = s.strip_suffix('Z').or_else(|| s.strip_suffix("+00:00"))?;
    // Drop fractional seconds if present.
    let s = match s.find('.') {
        Some(i) => &s[..i],
        None => s,
    };
    if s.len() != 19 {
        return None;
    }
    let b = s.as_bytes();
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let year: i64 = s[0..4].parse().ok()?;
    let month: i64 = s[5..7].parse().ok()?;
    let day: i64 = s[8..10].parse().ok()?;
    let hour: i64 = s[11..13].parse().ok()?;
    let min: i64 = s[14..16].parse().ok()?;
    let sec: i64 = s[17..19].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 60
    {
        return None;
    }
    let secs = days_from_civil(year, month, day) * 86400 + hour * 3600 + min * 60 + sec;
    if secs < 0 {
        return None;
    }
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs as u64))
}

/// Howard Hinnant's `days_from_civil` — days from 1970-01-01 to (y, m, d).
/// Public-domain algorithm. Handles negative years and all months.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn header(name: &str, value: &str) -> (reqwest::header::HeaderName, HeaderValue) {
        (
            name.parse().unwrap(),
            HeaderValue::from_str(value).unwrap(),
        )
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            let (name, value) = header(k, v);
            h.insert(name, value);
        }
        h
    }

    #[test]
    fn parses_rfc3339_with_trailing_z() {
        let t = parse_rfc3339_utc("2024-01-15T00:00:00Z").unwrap();
        let secs = t.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(secs, 19737 * 86400);
    }

    #[test]
    fn parses_rfc3339_with_fractional_seconds() {
        let t = parse_rfc3339_utc("2024-01-15T00:00:00.123Z").unwrap();
        let secs = t.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(secs, 19737 * 86400);
    }

    #[test]
    fn rejects_garbage_dates() {
        assert!(parse_rfc3339_utc("not a date").is_none());
        assert!(parse_rfc3339_utc("2024-13-01T00:00:00Z").is_none());
        assert!(parse_rfc3339_utc("2024-01-15 00:00:00Z").is_none());
    }

    #[test]
    fn snapshot_reads_present_headers_skips_missing() {
        let h = headers(&[
            ("anthropic-ratelimit-requests-remaining", "42"),
            ("anthropic-ratelimit-requests-reset", "2024-01-15T00:00:00Z"),
            ("anthropic-ratelimit-input-tokens-remaining", "0"),
            // output-tokens-* intentionally absent
        ]);
        let s = RateLimitSnapshot::from_headers(&h);
        assert_eq!(s.requests_remaining, Some(42));
        assert!(s.requests_reset.is_some());
        assert_eq!(s.input_tokens_remaining, Some(0));
        assert!(s.input_tokens_reset.is_none());
        assert!(s.output_tokens_remaining.is_none());
    }

    #[test]
    fn wait_returns_none_when_nothing_exhausted() {
        let s = RateLimitSnapshot {
            requests_remaining: Some(10),
            ..Default::default()
        };
        assert!(s.wait_until_reset(SystemTime::now(), Duration::ZERO).is_none());
    }

    #[test]
    fn wait_picks_longest_exhausted_reset() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let s = RateLimitSnapshot {
            requests_remaining: Some(0),
            requests_reset: Some(now + Duration::from_secs(5)),
            input_tokens_remaining: Some(0),
            input_tokens_reset: Some(now + Duration::from_secs(20)),
            output_tokens_remaining: Some(7), // not exhausted; ignored
            output_tokens_reset: Some(now + Duration::from_secs(60)),
            ..Default::default()
        };
        let wait = s.wait_until_reset(now, Duration::from_millis(500)).unwrap();
        // 20s from the input-tokens reset + 500ms safety.
        assert_eq!(wait, Duration::from_millis(20_500));
    }

    #[test]
    fn wait_clamps_past_resets_to_safety_margin() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let s = RateLimitSnapshot {
            requests_remaining: Some(0),
            requests_reset: Some(now - Duration::from_secs(5)), // already past
            ..Default::default()
        };
        let wait = s.wait_until_reset(now, Duration::from_millis(500)).unwrap();
        // Past reset → duration_since returns 0; we still wait the safety margin.
        assert_eq!(wait, Duration::from_millis(500));
    }

    #[test]
    fn observer_round_trip() {
        let obs = RateLimitObserver::new();
        assert!(obs.current().is_none());
        let s = RateLimitSnapshot {
            requests_remaining: Some(3),
            ..Default::default()
        };
        obs.record(s.clone());
        assert_eq!(obs.current(), Some(s));
    }
}
