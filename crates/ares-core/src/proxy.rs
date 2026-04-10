//! Proxy rotation for anti-bot evasion.
//!
//! Provides a pool of proxy URLs with configurable rotation strategies.
//! Used by fetchers to distribute requests across multiple exit IPs.

use std::sync::atomic::{AtomicUsize, Ordering};

/// A single proxy endpoint.
#[derive(Debug, Clone)]
pub struct ProxyEntry {
    /// Full proxy URL (e.g., `http://host:port`, `socks5://host:port`).
    pub url: String,
    /// Optional username for authenticated proxies.
    pub username: Option<String>,
    /// Optional password for authenticated proxies.
    pub password: Option<String>,
}

impl ProxyEntry {
    /// Create a simple (unauthenticated) proxy entry.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            username: None,
            password: None,
        }
    }

    /// Create a proxy entry with authentication credentials.
    pub fn with_auth(url: impl Into<String>, username: &str, password: &str) -> Self {
        Self {
            url: url.into(),
            username: Some(username.to_string()),
            password: Some(password.to_string()),
        }
    }

    /// Returns the proxy URL with embedded credentials if present.
    ///
    /// Converts `http://host:port` + user/pass into `http://user:pass@host:port`.
    pub fn authenticated_url(&self) -> String {
        match (&self.username, &self.password) {
            (Some(user), Some(pass)) => {
                if let Some(rest) = self.url.strip_prefix("http://") {
                    format!("http://{user}:{pass}@{rest}")
                } else if let Some(rest) = self.url.strip_prefix("https://") {
                    format!("https://{user}:{pass}@{rest}")
                } else if let Some(rest) = self.url.strip_prefix("socks5://") {
                    format!("socks5://{user}:{pass}@{rest}")
                } else {
                    self.url.clone()
                }
            }
            _ => self.url.clone(),
        }
    }
}

/// Strategy for selecting the next proxy from the pool.
#[derive(Debug, Clone, Copy, Default)]
pub enum RotationStrategy {
    /// Cycle through proxies in order (0, 1, 2, …, 0, 1, 2, …).
    #[default]
    RoundRobin,
    /// Pick a random proxy each time.
    Random,
}

impl std::str::FromStr for RotationStrategy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "round-robin" | "roundrobin" | "rr" => Ok(Self::RoundRobin),
            "random" | "rand" => Ok(Self::Random),
            _ => Err(format!(
                "Unknown rotation strategy '{s}'. Expected: round-robin, random"
            )),
        }
    }
}

impl std::fmt::Display for RotationStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RoundRobin => write!(f, "round-robin"),
            Self::Random => write!(f, "random"),
        }
    }
}

/// A pool of proxies with rotation.
///
/// Thread-safe: the round-robin index uses `AtomicUsize`, so multiple tasks
/// can call [`next`](Self::next) concurrently without locking.
#[derive(Debug)]
pub struct ProxyConfig {
    proxies: Vec<ProxyEntry>,
    rotation: RotationStrategy,
    index: AtomicUsize,
}

impl ProxyConfig {
    /// Create a new proxy configuration.
    ///
    /// # Panics
    ///
    /// Panics if `proxies` is empty.
    pub fn new(proxies: Vec<ProxyEntry>, rotation: RotationStrategy) -> Self {
        assert!(
            !proxies.is_empty(),
            "ProxyConfig requires at least one proxy"
        );
        Self {
            proxies,
            rotation,
            index: AtomicUsize::new(0),
        }
    }

    /// Parse a newline-delimited proxy list (one URL per line).
    ///
    /// Blank lines and lines starting with `#` are ignored.
    pub fn from_lines(text: &str, rotation: RotationStrategy) -> Result<Self, String> {
        let proxies: Vec<ProxyEntry> = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(ProxyEntry::new)
            .collect();
        if proxies.is_empty() {
            return Err("No proxies found in input".to_string());
        }
        Ok(Self::new(proxies, rotation))
    }

    /// Returns the next proxy according to the rotation strategy.
    pub fn next(&self) -> &ProxyEntry {
        let idx = self.next_index();
        &self.proxies[idx]
    }

    /// Returns the index of the next proxy to use.
    ///
    /// Useful when you have a parallel data structure (e.g., pre-built clients)
    /// indexed the same way as the proxy list.
    pub fn next_index(&self) -> usize {
        match self.rotation {
            RotationStrategy::RoundRobin => {
                self.index.fetch_add(1, Ordering::Relaxed) % self.proxies.len()
            }
            RotationStrategy::Random => random_index(self.proxies.len()),
        }
    }

    /// Number of proxies in the pool.
    pub fn len(&self) -> usize {
        self.proxies.len()
    }

    /// Returns `true` if the pool is empty (should never happen after construction).
    pub fn is_empty(&self) -> bool {
        self.proxies.is_empty()
    }
}

impl Clone for ProxyConfig {
    fn clone(&self) -> Self {
        Self {
            proxies: self.proxies.clone(),
            rotation: self.rotation,
            index: AtomicUsize::new(self.index.load(Ordering::Relaxed)),
        }
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
    fn round_robin_cycles() {
        let proxies = vec![
            ProxyEntry::new("http://p1:8080"),
            ProxyEntry::new("http://p2:8080"),
            ProxyEntry::new("http://p3:8080"),
        ];
        let config = ProxyConfig::new(proxies, RotationStrategy::RoundRobin);

        assert_eq!(config.next().url, "http://p1:8080");
        assert_eq!(config.next().url, "http://p2:8080");
        assert_eq!(config.next().url, "http://p3:8080");
        // Wraps around
        assert_eq!(config.next().url, "http://p1:8080");
    }

    #[test]
    fn random_selects_valid_entry() {
        let proxies = vec![
            ProxyEntry::new("http://p1:8080"),
            ProxyEntry::new("http://p2:8080"),
        ];
        let config = ProxyConfig::new(proxies, RotationStrategy::Random);

        for _ in 0..50 {
            let proxy = config.next();
            assert!(
                proxy.url == "http://p1:8080" || proxy.url == "http://p2:8080",
                "unexpected proxy: {}",
                proxy.url
            );
        }
    }

    #[test]
    fn authenticated_url_embeds_credentials() {
        let entry = ProxyEntry::with_auth("http://proxy:8080", "user", "pass");
        assert_eq!(entry.authenticated_url(), "http://user:pass@proxy:8080");

        let entry = ProxyEntry::with_auth("socks5://proxy:1080", "u", "p");
        assert_eq!(entry.authenticated_url(), "socks5://u:p@proxy:1080");
    }

    #[test]
    fn authenticated_url_no_creds() {
        let entry = ProxyEntry::new("http://proxy:8080");
        assert_eq!(entry.authenticated_url(), "http://proxy:8080");
    }

    #[test]
    fn from_lines_parses_correctly() {
        let input = "
# Comment line
http://p1:8080
http://p2:8080

http://p3:8080
";
        let config = ProxyConfig::from_lines(input, RotationStrategy::RoundRobin).unwrap();
        assert_eq!(config.len(), 3);
    }

    #[test]
    fn from_lines_rejects_empty() {
        let result = ProxyConfig::from_lines("# only comments", RotationStrategy::RoundRobin);
        assert!(result.is_err());
    }

    #[test]
    fn rotation_strategy_from_str() {
        assert!(matches!(
            "round-robin".parse::<RotationStrategy>().unwrap(),
            RotationStrategy::RoundRobin
        ));
        assert!(matches!(
            "random".parse::<RotationStrategy>().unwrap(),
            RotationStrategy::Random
        ));
        assert!("invalid".parse::<RotationStrategy>().is_err());
    }

    #[test]
    #[should_panic(expected = "at least one proxy")]
    fn panics_on_empty_proxies() {
        ProxyConfig::new(vec![], RotationStrategy::RoundRobin);
    }

    #[test]
    fn clone_preserves_state() {
        let proxies = vec![
            ProxyEntry::new("http://p1:8080"),
            ProxyEntry::new("http://p2:8080"),
        ];
        let config = ProxyConfig::new(proxies, RotationStrategy::RoundRobin);
        config.next(); // advance to index 1

        let cloned = config.clone();
        // Clone should start from the same position
        assert_eq!(cloned.next().url, "http://p2:8080");
    }
}
