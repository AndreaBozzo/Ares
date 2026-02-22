//! Per-domain request throttling for polite fetching.
//!
//! Wraps any [`Fetcher`] implementation with configurable per-domain delays
//! to prevent hammering target sites. Essential for crawling (spidering)
//! workloads where many URLs share the same host, and recommended as a
//! baseline politeness measure for all production use.
//!
//! # Example
//!
//! ```rust,no_run
//! use std::time::Duration;
//! use ares_core::throttle::{ThrottledFetcher, ThrottleConfig};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! // Wrap any Fetcher with a 1-second per-domain delay and ±500ms jitter
//! # use ares_core::traits::Fetcher;
//! # #[derive(Clone)] struct MyFetcher;
//! # impl Fetcher for MyFetcher {
//! #     async fn fetch(&self, _: &str) -> Result<String, ares_core::error::AppError> { todo!() }
//! # }
//! let inner = MyFetcher;
//! let config = ThrottleConfig::new(Duration::from_secs(1))
//!     .with_jitter(Duration::from_millis(500));
//! let fetcher = ThrottledFetcher::new(inner, config);
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use url::Url;

use crate::error::AppError;
use crate::traits::Fetcher;

/// Configuration for the throttled fetcher.
#[derive(Debug, Clone)]
pub struct ThrottleConfig {
    /// Minimum delay between consecutive requests to the same domain.
    pub delay: Duration,

    /// Maximum random jitter added on top of `delay` (uniform [0, jitter]).
    ///
    /// Randomises request timing to appear more human-like.
    /// Set to `Duration::ZERO` to disable.
    pub jitter: Duration,
}

impl ThrottleConfig {
    /// Create a new config with the given per-domain delay and no jitter.
    pub fn new(delay: Duration) -> Self {
        Self {
            delay,
            jitter: Duration::ZERO,
        }
    }

    /// Add random jitter (uniform [0, jitter]) on top of the base delay.
    pub fn with_jitter(mut self, jitter: Duration) -> Self {
        self.jitter = jitter;
        self
    }

    /// Compute the effective delay for a single wait (delay + random jitter).
    fn effective_delay(&self) -> Duration {
        if self.jitter.is_zero() {
            return self.delay;
        }
        let jitter_ms = rand_jitter_ms(self.jitter.as_millis() as u64);
        self.delay + Duration::from_millis(jitter_ms)
    }
}

impl Default for ThrottleConfig {
    /// 1 second delay, 500ms jitter — a sensible default for polite crawling.
    fn default() -> Self {
        Self {
            delay: Duration::from_secs(1),
            jitter: Duration::from_millis(500),
        }
    }
}

/// A [`Fetcher`] wrapper that enforces per-domain throttling.
///
/// Tracks the last request time for each domain (scheme + host + port)
/// and sleeps before making a new request if the minimum delay hasn't
/// elapsed. Thread-safe: multiple tasks can call `fetch` concurrently
/// and the throttle will serialise access per domain.
#[derive(Clone)]
pub struct ThrottledFetcher<F> {
    inner: F,
    config: ThrottleConfig,
    /// Last request time per domain key.
    last_request: Arc<Mutex<HashMap<String, Instant>>>,
}

impl<F: Fetcher> ThrottledFetcher<F> {
    /// Wrap an existing fetcher with throttling.
    pub fn new(inner: F, config: ThrottleConfig) -> Self {
        Self {
            inner,
            config,
            last_request: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Extract the domain key from a URL (scheme://host:port).
    fn domain_key(url_str: &str) -> Option<String> {
        let url = Url::parse(url_str).ok()?;
        let host = url.host_str()?;
        let port = url
            .port_or_known_default()
            .map(|p| format!(":{p}"))
            .unwrap_or_default();
        Some(format!("{}://{}{}", url.scheme(), host, port))
    }

    /// Wait until the per-domain delay has elapsed, then record the
    /// current time as the last request for this domain.
    async fn wait_for_domain(&self, domain: &str) {
        let mut map = self.last_request.lock().await;

        if let Some(&last) = map.get(domain) {
            let elapsed = last.elapsed();
            let required = self.config.effective_delay();
            if elapsed < required {
                let sleep_duration = required - elapsed;
                // Drop the lock while sleeping so other domains aren't blocked.
                drop(map);
                tracing::debug!(
                    domain = %domain,
                    sleep_ms = %sleep_duration.as_millis(),
                    "Throttling request"
                );
                tokio::time::sleep(sleep_duration).await;
                // Re-acquire and update.
                let mut map = self.last_request.lock().await;
                map.insert(domain.to_string(), Instant::now());
            } else {
                map.insert(domain.to_string(), Instant::now());
            }
        } else {
            map.insert(domain.to_string(), Instant::now());
        }
    }
}

impl<F: Fetcher> Fetcher for ThrottledFetcher<F> {
    async fn fetch(&self, url: &str) -> Result<String, AppError> {
        if let Some(domain) = Self::domain_key(url) {
            self.wait_for_domain(&domain).await;
        }
        self.inner.fetch(url).await
    }
}

// ---------------------------------------------------------------------------
// Deterministic jitter based on std — avoids pulling in the `rand` crate.
// Uses a simple xorshift seeded from the current time.
// ---------------------------------------------------------------------------

fn rand_jitter_ms(max_ms: u64) -> u64 {
    if max_ms == 0 {
        return 0;
    }
    // Seed from high-resolution clock — good enough for jitter, not crypto.
    let mut x = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    // xorshift64
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x % max_ms
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::MockFetcher;

    #[test]
    fn domain_key_extracts_correctly() {
        assert_eq!(
            ThrottledFetcher::<MockFetcher>::domain_key("https://example.com/path?q=1"),
            Some("https://example.com:443".to_string())
        );
        assert_eq!(
            ThrottledFetcher::<MockFetcher>::domain_key("http://example.com:8080/page"),
            Some("http://example.com:8080".to_string())
        );
        assert_eq!(
            ThrottledFetcher::<MockFetcher>::domain_key("http://example.com"),
            Some("http://example.com:80".to_string())
        );
    }

    #[test]
    fn domain_key_returns_none_for_invalid_url() {
        assert_eq!(
            ThrottledFetcher::<MockFetcher>::domain_key("not-a-url"),
            None
        );
    }

    #[test]
    fn effective_delay_without_jitter() {
        let config = ThrottleConfig::new(Duration::from_secs(1));
        assert_eq!(config.effective_delay(), Duration::from_secs(1));
    }

    #[test]
    fn effective_delay_with_jitter_is_bounded() {
        let config =
            ThrottleConfig::new(Duration::from_millis(100)).with_jitter(Duration::from_millis(50));
        for _ in 0..100 {
            let d = config.effective_delay();
            assert!(d >= Duration::from_millis(100));
            assert!(d < Duration::from_millis(150));
        }
    }

    #[tokio::test]
    async fn throttle_enforces_delay_on_same_domain() {
        let inner = MockFetcher::new("<html>ok</html>");
        let config = ThrottleConfig::new(Duration::from_millis(100));
        let fetcher = ThrottledFetcher::new(inner, config);

        let start = Instant::now();
        fetcher.fetch("http://example.com/page1").await.unwrap();
        fetcher.fetch("http://example.com/page2").await.unwrap();
        let elapsed = start.elapsed();

        assert!(
            elapsed >= Duration::from_millis(100),
            "Second request should have been delayed by at least 100ms, elapsed: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn throttle_does_not_delay_different_domains() {
        let inner = MockFetcher::new("<html>ok</html>");
        let config = ThrottleConfig::new(Duration::from_millis(200));
        let fetcher = ThrottledFetcher::new(inner, config);

        let start = Instant::now();
        fetcher.fetch("http://example.com/page1").await.unwrap();
        fetcher.fetch("http://other.com/page1").await.unwrap();
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_millis(150),
            "Different domains should not be throttled against each other, elapsed: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn throttle_passes_through_fetch_result() {
        let inner = MockFetcher::new("<html>hello</html>");
        let config = ThrottleConfig::new(Duration::from_millis(0));
        let fetcher = ThrottledFetcher::new(inner, config);

        let result = fetcher.fetch("http://example.com").await.unwrap();
        assert_eq!(result, "<html>hello</html>");
    }

    #[tokio::test]
    async fn throttle_passes_through_errors() {
        let inner = MockFetcher::with_error(AppError::HttpError("fail".into()));
        let config = ThrottleConfig::new(Duration::from_millis(0));
        let fetcher = ThrottledFetcher::new(inner, config);

        let err = fetcher.fetch("http://example.com").await.unwrap_err();
        assert!(matches!(err, AppError::HttpError(_)));
    }

    #[test]
    fn default_config_is_sensible() {
        let config = ThrottleConfig::default();
        assert_eq!(config.delay, Duration::from_secs(1));
        assert_eq!(config.jitter, Duration::from_millis(500));
    }
}
