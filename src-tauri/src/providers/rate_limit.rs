//! Proactive rate-limit pacing for upstream providers.
//!
//! Each base provider parses the rate-limit headers from its own responses
//! into a [`RateLimitSnapshot`] and records it into a shared [`MultiObserver`]
//! keyed by provider name. [`RateLimitedProvider`] sits in the decorator
//! stack and, before each call, looks up the snapshot for the provider that
//! will actually handle the request (derived from `req.model`). If a counter
//! has hit zero or fallen below the low-water mark, it sleeps until reset —
//! turning a soon-to-be-429 into a clean wait, instead of the cascading retry
//! storm that happens when many concurrent calls all 429 at once. Stack
//! position is documented in CLAUDE.md.

use std::collections::HashMap;
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
    pub requests_limit: Option<u64>,
    pub requests_remaining: Option<u64>,
    pub requests_reset: Option<SystemTime>,
    pub input_tokens_limit: Option<u64>,
    pub input_tokens_remaining: Option<u64>,
    pub input_tokens_reset: Option<SystemTime>,
    pub output_tokens_limit: Option<u64>,
    pub output_tokens_remaining: Option<u64>,
    pub output_tokens_reset: Option<SystemTime>,
}

impl RateLimitSnapshot {
    /// Read the `anthropic-ratelimit-*` headers from a response. Doesn't
    /// consume the response body.
    pub fn from_anthropic_headers(h: &HeaderMap) -> Self {
        Self {
            requests_limit: parse_u64(h, "anthropic-ratelimit-requests-limit"),
            requests_remaining: parse_u64(h, "anthropic-ratelimit-requests-remaining"),
            requests_reset: parse_rfc3339_reset(h, "anthropic-ratelimit-requests-reset"),
            input_tokens_limit: parse_u64(h, "anthropic-ratelimit-input-tokens-limit"),
            input_tokens_remaining: parse_u64(h, "anthropic-ratelimit-input-tokens-remaining"),
            input_tokens_reset: parse_rfc3339_reset(h, "anthropic-ratelimit-input-tokens-reset"),
            output_tokens_limit: parse_u64(h, "anthropic-ratelimit-output-tokens-limit"),
            output_tokens_remaining: parse_u64(h, "anthropic-ratelimit-output-tokens-remaining"),
            output_tokens_reset: parse_rfc3339_reset(h, "anthropic-ratelimit-output-tokens-reset"),
        }
    }

    /// Read the OpenAI `x-ratelimit-*` headers from a response. OpenAI
    /// exposes only a requests bucket and a single tokens bucket (not split
    /// into input/output). We map the tokens bucket to the `input_tokens_*`
    /// slot — the conservative one already used by pacing logic. The
    /// `output_tokens_*` triple stays `None`. Reset values are duration
    /// strings like `"6s"`, `"1m30s"`, `"500ms"`, so we resolve them to an
    /// absolute `SystemTime` via `now + parsed`.
    pub fn from_openai_headers(h: &HeaderMap, now: SystemTime) -> Self {
        Self {
            requests_limit: parse_u64(h, "x-ratelimit-limit-requests"),
            requests_remaining: parse_u64(h, "x-ratelimit-remaining-requests"),
            requests_reset: parse_duration_reset(h, "x-ratelimit-reset-requests", now),
            input_tokens_limit: parse_u64(h, "x-ratelimit-limit-tokens"),
            input_tokens_remaining: parse_u64(h, "x-ratelimit-remaining-tokens"),
            input_tokens_reset: parse_duration_reset(h, "x-ratelimit-reset-tokens", now),
            output_tokens_limit: None,
            output_tokens_remaining: None,
            output_tokens_reset: None,
        }
    }

    /// How long to wait before the next call. `None` means no counter has
    /// tripped — caller can proceed immediately. Otherwise returns the longest
    /// reset wait among tripped counters plus `safety`.
    ///
    /// A counter trips when:
    ///   - `remaining == 0` (hard exhaustion — always trips), OR
    ///   - `remaining` is below `limit * low_water_fraction` (soft trigger;
    ///     requires the `-limit` header to be present).
    ///
    /// `low_water_fraction = 0.0` reproduces the hard-only behavior. The
    /// safety margin (a few hundred ms) absorbs clock skew so we don't fire
    /// the instant the bucket nominally refills.
    pub fn wait_until_reset(
        &self,
        now: SystemTime,
        safety: Duration,
        low_water_fraction: f64,
    ) -> Option<Duration> {
        let mut max_wait: Option<Duration> = None;
        let triples = [
            (self.requests_remaining, self.requests_limit, self.requests_reset),
            (self.input_tokens_remaining, self.input_tokens_limit, self.input_tokens_reset),
            (self.output_tokens_remaining, self.output_tokens_limit, self.output_tokens_reset),
        ];
        for (rem, lim, reset_at) in triples {
            let Some(reset_at) = reset_at else { continue };
            let Some(rem) = rem else { continue };
            let trip = rem == 0
                || lim
                    .map(|l| (rem as f64) < (l as f64) * low_water_fraction)
                    .unwrap_or(false);
            if trip {
                let dur = reset_at.duration_since(now).unwrap_or(Duration::ZERO) + safety;
                max_wait = Some(max_wait.map_or(dur, |m| m.max(dur)));
            }
        }
        max_wait
    }
}

/// Shared cell holding the latest snapshot per provider. Writers: each base
/// provider (Anthropic, OpenAI, ...) records under its stable key. Reader:
/// `RateLimitedProvider` looks up the snapshot for the provider that will
/// handle the next call (chosen via `req.model`).
#[derive(Debug, Default)]
pub struct MultiObserver {
    snapshots: Mutex<HashMap<&'static str, RateLimitSnapshot>>,
}

impl MultiObserver {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn record(&self, provider: &'static str, snapshot: RateLimitSnapshot) {
        self.snapshots.lock().unwrap().insert(provider, snapshot);
    }

    pub fn current(&self, provider: &str) -> Option<RateLimitSnapshot> {
        self.snapshots.lock().unwrap().get(provider).cloned()
    }
}

/// Function used to map a model id to the provider key that will handle it.
/// Same logic that the multiplex provider uses for dispatch — kept here as
/// a fn pointer so the pacing decorator can read the right snapshot without
/// taking a dependency on the multiplex.
pub type ModelRouter = fn(&str) -> &'static str;

/// `Provider` decorator that sleeps before each call when the most recent
/// snapshot (for the routed provider) shows a tripped counter (exhausted or
/// below `low_water_fraction`). Sleeps cap at `max_wait` to bound
/// pathological clock skew or stale snapshots.
pub struct RateLimitedProvider {
    inner: Arc<dyn Provider>,
    observer: Arc<MultiObserver>,
    router: ModelRouter,
    safety: Duration,
    max_wait: Duration,
    low_water_fraction: f64,
}

impl RateLimitedProvider {
    /// Conservative low-water default: pace when fewer than 5% of any counter
    /// is left. Tuned to absorb a burst of concurrent calls without going
    /// negative — most stages fire 4–8 calls in parallel, so 5% of a typical
    /// per-minute budget is the right neighborhood.
    const DEFAULT_LOW_WATER: f64 = 0.05;

    pub fn new(
        inner: Arc<dyn Provider>,
        observer: Arc<MultiObserver>,
        router: ModelRouter,
    ) -> Self {
        Self {
            inner,
            observer,
            router,
            safety: Duration::from_millis(500),
            max_wait: Duration::from_secs(120),
            low_water_fraction: Self::DEFAULT_LOW_WATER,
        }
    }

    pub fn with_low_water_fraction(mut self, fraction: f64) -> Self {
        self.low_water_fraction = fraction.clamp(0.0, 1.0);
        self
    }
}

#[async_trait]
impl Provider for RateLimitedProvider {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    async fn generate(&self, req: GenerationRequest) -> ProviderResult<Response> {
        let provider_key = (self.router)(&req.model);
        if let Some(snap) = self.observer.current(provider_key) {
            if let Some(wait) =
                snap.wait_until_reset(SystemTime::now(), self.safety, self.low_water_fraction)
            {
                let capped = wait.min(self.max_wait);
                if capped >= Duration::from_millis(50) {
                    info!(
                        provider = provider_key,
                        wait_ms = capped.as_millis() as u64,
                        "rate-limit pacing before next call"
                    );
                    tokio::time::sleep(capped).await;
                }
                if wait > self.max_wait {
                    warn!(
                        provider = provider_key,
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

fn parse_rfc3339_reset(h: &HeaderMap, name: &str) -> Option<SystemTime> {
    parse_rfc3339_utc(h.get(name)?.to_str().ok()?)
}

fn parse_duration_reset(h: &HeaderMap, name: &str, now: SystemTime) -> Option<SystemTime> {
    let d = parse_openai_duration(h.get(name)?.to_str().ok()?)?;
    Some(now + d)
}

/// Parse OpenAI's compact rate-limit duration strings like `"6s"`,
/// `"1m30s"`, `"500ms"`, `"1h2m3s"`. Returns `None` on any unexpected input.
/// Units recognized: `ms`, `s`, `m`, `h`. The `ms` suffix is checked before
/// `s`/`m` so `"500ms"` doesn't misparse as `500s`.
fn parse_openai_duration(s: &str) -> Option<Duration> {
    let bytes = s.trim().as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let mut total = Duration::ZERO;
    let mut i = 0;
    while i < bytes.len() {
        // Read the number.
        let num_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == num_start {
            return None;
        }
        let n: u64 = std::str::from_utf8(&bytes[num_start..i]).ok()?.parse().ok()?;
        // Read the unit. `ms` first; then single-char units.
        if i + 2 <= bytes.len() && &bytes[i..i + 2] == b"ms" {
            total += Duration::from_millis(n);
            i += 2;
        } else if i < bytes.len() {
            let unit_secs: u64 = match bytes[i] {
                b's' => 1,
                b'm' => 60,
                b'h' => 3600,
                _ => return None,
            };
            total = total.checked_add(Duration::from_secs(n.checked_mul(unit_secs)?))?;
            i += 1;
        } else {
            return None;
        }
    }
    Some(total)
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
    fn snapshot_reads_present_anthropic_headers_skips_missing() {
        let h = headers(&[
            ("anthropic-ratelimit-requests-remaining", "42"),
            ("anthropic-ratelimit-requests-reset", "2024-01-15T00:00:00Z"),
            ("anthropic-ratelimit-input-tokens-remaining", "0"),
            // output-tokens-* intentionally absent
        ]);
        let s = RateLimitSnapshot::from_anthropic_headers(&h);
        assert_eq!(s.requests_remaining, Some(42));
        assert!(s.requests_reset.is_some());
        assert_eq!(s.input_tokens_remaining, Some(0));
        assert!(s.input_tokens_reset.is_none());
        assert!(s.output_tokens_remaining.is_none());
    }

    #[test]
    fn snapshot_reads_present_openai_headers_skips_missing() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let h = headers(&[
            ("x-ratelimit-limit-requests", "100"),
            ("x-ratelimit-remaining-requests", "75"),
            ("x-ratelimit-reset-requests", "6s"),
            ("x-ratelimit-remaining-tokens", "0"),
            ("x-ratelimit-reset-tokens", "1m30s"),
            // limit-tokens absent
        ]);
        let s = RateLimitSnapshot::from_openai_headers(&h, now);
        assert_eq!(s.requests_limit, Some(100));
        assert_eq!(s.requests_remaining, Some(75));
        assert_eq!(s.requests_reset, Some(now + Duration::from_secs(6)));
        assert_eq!(s.input_tokens_remaining, Some(0));
        assert_eq!(s.input_tokens_reset, Some(now + Duration::from_secs(90)));
        // OpenAI doesn't split tokens into input/output; output stays None.
        assert!(s.output_tokens_remaining.is_none());
        assert!(s.output_tokens_reset.is_none());
    }

    #[test]
    fn parses_openai_reset_durations() {
        assert_eq!(parse_openai_duration("6s"), Some(Duration::from_secs(6)));
        assert_eq!(
            parse_openai_duration("1m30s"),
            Some(Duration::from_secs(90))
        );
        assert_eq!(
            parse_openai_duration("500ms"),
            Some(Duration::from_millis(500))
        );
        assert_eq!(
            parse_openai_duration("1h2m3s"),
            Some(Duration::from_secs(3723))
        );
        assert_eq!(parse_openai_duration("0s"), Some(Duration::ZERO));
        // ms must be checked before s — otherwise "500ms" would misparse.
        assert_eq!(
            parse_openai_duration("100ms"),
            Some(Duration::from_millis(100))
        );
        // Garbage inputs return None.
        assert_eq!(parse_openai_duration(""), None);
        assert_eq!(parse_openai_duration("abc"), None);
        assert_eq!(parse_openai_duration("12"), None); // no unit
        assert_eq!(parse_openai_duration("12x"), None); // unknown unit
    }

    #[test]
    fn wait_returns_none_when_nothing_tripped() {
        let s = RateLimitSnapshot {
            requests_remaining: Some(10),
            ..Default::default()
        };
        assert!(s
            .wait_until_reset(SystemTime::now(), Duration::ZERO, 0.0)
            .is_none());
    }

    #[test]
    fn wait_picks_longest_exhausted_reset() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let s = RateLimitSnapshot {
            requests_remaining: Some(0),
            requests_reset: Some(now + Duration::from_secs(5)),
            input_tokens_remaining: Some(0),
            input_tokens_reset: Some(now + Duration::from_secs(20)),
            output_tokens_remaining: Some(7), // not exhausted; ignored at fraction=0
            output_tokens_reset: Some(now + Duration::from_secs(60)),
            ..Default::default()
        };
        let wait = s
            .wait_until_reset(now, Duration::from_millis(500), 0.0)
            .unwrap();
        // 20s from the input-tokens reset + 500ms safety.
        assert_eq!(wait, Duration::from_millis(20_500));
    }

    #[test]
    fn wait_clamps_past_resets_to_safety_margin() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let s = RateLimitSnapshot {
            requests_remaining: Some(0),
            requests_reset: Some(now - Duration::from_secs(5)),
            ..Default::default()
        };
        let wait = s
            .wait_until_reset(now, Duration::from_millis(500), 0.0)
            .unwrap();
        assert_eq!(wait, Duration::from_millis(500));
    }

    #[test]
    fn low_water_trips_before_full_exhaustion() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        // 40 of 1000 remaining (4%) — under a 5% low-water threshold.
        let s = RateLimitSnapshot {
            input_tokens_limit: Some(1000),
            input_tokens_remaining: Some(40),
            input_tokens_reset: Some(now + Duration::from_secs(10)),
            ..Default::default()
        };
        // At fraction=0.0 (hard-only), 40 > 0 so nothing trips.
        assert!(s
            .wait_until_reset(now, Duration::from_millis(500), 0.0)
            .is_none());
        // At fraction=0.05 (5%), 40/1000 = 4% — trips.
        let wait = s
            .wait_until_reset(now, Duration::from_millis(500), 0.05)
            .unwrap();
        assert_eq!(wait, Duration::from_millis(10_500));
    }

    #[test]
    fn low_water_ignored_when_limit_header_missing() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let s = RateLimitSnapshot {
            // No `*_limit` populated; we can't compute the fraction so we
            // fall back to hard-only behavior.
            input_tokens_remaining: Some(40),
            input_tokens_reset: Some(now + Duration::from_secs(10)),
            ..Default::default()
        };
        assert!(s
            .wait_until_reset(now, Duration::from_millis(500), 0.5)
            .is_none());
    }

    #[test]
    fn multi_observer_per_provider_isolation() {
        let obs = MultiObserver::new();
        assert!(obs.current("anthropic").is_none());
        assert!(obs.current("openai").is_none());

        let anth_snap = RateLimitSnapshot {
            requests_remaining: Some(3),
            ..Default::default()
        };
        obs.record("anthropic", anth_snap.clone());

        assert_eq!(obs.current("anthropic"), Some(anth_snap));
        assert!(
            obs.current("openai").is_none(),
            "recording anthropic must not leak into openai's slot"
        );

        // Recording for a different provider doesn't displace the first.
        let oai_snap = RateLimitSnapshot {
            input_tokens_remaining: Some(0),
            ..Default::default()
        };
        obs.record("openai", oai_snap.clone());
        assert!(obs.current("anthropic").is_some());
        assert_eq!(obs.current("openai"), Some(oai_snap));
    }
}
