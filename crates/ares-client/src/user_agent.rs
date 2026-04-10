//! Realistic User-Agent rotation for anti-bot evasion.
//!
//! Provides a curated pool of browser User-Agent strings that can be
//! rotated per-request to avoid fingerprinting based on static UA headers.

/// Curated pool of realistic, recent browser User-Agent strings.
///
/// Covers Chrome, Firefox, Safari, and Edge across Windows, macOS, and Linux.
const USER_AGENTS: &[&str] = &[
    // Chrome on Windows
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36",
    // Chrome on macOS
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36",
    // Chrome on Linux
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36",
    // Firefox on Windows
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:133.0) Gecko/20100101 Firefox/133.0",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:132.0) Gecko/20100101 Firefox/132.0",
    // Firefox on macOS
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:133.0) Gecko/20100101 Firefox/133.0",
    // Firefox on Linux
    "Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0",
    "Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:132.0) Gecko/20100101 Firefox/132.0",
    // Safari on macOS
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.6 Safari/605.1.15",
    // Edge on Windows
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0",
    // Edge on macOS
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0",
    // Chrome on Android (mobile)
    "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36",
    // Safari on iOS (mobile)
    "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1",
    // Samsung Internet
    "Mozilla/5.0 (Linux; Android 14; SM-S928B) AppleWebKit/537.36 (KHTML, like Gecko) SamsungBrowser/25.0 Chrome/121.0.0.0 Mobile Safari/537.36",
];

/// A pool of User-Agent strings for rotation.
///
/// Each call to [`next`](Self::next) returns a different User-Agent string,
/// selected randomly from the built-in pool.
#[derive(Debug, Clone)]
pub struct UserAgentPool;

impl UserAgentPool {
    /// Returns a randomly selected User-Agent string from the built-in pool.
    pub fn next(&self) -> &'static str {
        USER_AGENTS[random_index(USER_AGENTS.len())]
    }
}

/// Simple random index without pulling in the `rand` crate.
fn random_index(len: usize) -> usize {
    let mut x = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    // xorshift64
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (x as usize) % len
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_returns_valid_ua() {
        let pool = UserAgentPool;
        for _ in 0..50 {
            let ua = pool.next();
            assert!(ua.starts_with("Mozilla/5.0"), "unexpected UA: {ua}");
            assert!(ua.len() > 50, "UA too short: {ua}");
        }
    }

    #[test]
    fn pool_has_sufficient_variety() {
        let pool = UserAgentPool;
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            seen.insert(pool.next());
        }
        // With 20 UAs and 200 draws, we should see at least a few distinct ones
        assert!(
            seen.len() >= 3,
            "Expected variety, only got {} distinct UAs",
            seen.len()
        );
    }

    #[test]
    fn builtin_pool_size() {
        assert_eq!(USER_AGENTS.len(), 20);
    }
}
