use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use url::Url;

use ares_core::traits::RobotsChecker;

/// robots.txt checker with per-domain caching.
///
/// Fetches and parses robots.txt once per domain, caching results
/// for the lifetime of this instance. Fetch or parse failures
/// are treated as "allow all" (graceful degradation).
#[derive(Clone)]
pub struct CachedRobotsChecker {
    client: reqwest::Client,
    user_agent: String,
    /// Cache: domain → parsed robots.txt content (None = fetch failed, allow all)
    cache: Arc<Mutex<HashMap<String, Option<String>>>>,
}

impl CachedRobotsChecker {
    pub fn new(client: reqwest::Client, user_agent: impl Into<String>) -> Self {
        Self {
            client,
            user_agent: user_agent.into(),
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Create with default reqwest client and the given user-agent.
    pub fn with_user_agent(user_agent: impl Into<String>) -> Self {
        Self::new(reqwest::Client::new(), user_agent)
    }
}

impl RobotsChecker for CachedRobotsChecker {
    async fn is_allowed(&self, url: &str) -> bool {
        let parsed = match Url::parse(url) {
            Ok(u) => u,
            Err(_) => return true, // Can't parse URL, allow
        };

        let origin = format!(
            "{}://{}",
            parsed.scheme(),
            match parsed.host_str() {
                Some(h) => {
                    if let Some(port) = parsed.port() {
                        format!("{h}:{port}")
                    } else {
                        h.to_string()
                    }
                }
                None => return true,
            }
        );

        // Check cache
        let cached = {
            let cache = self.cache.lock().unwrap();
            cache.get(&origin).cloned()
        };

        let robots_content = match cached {
            Some(content) => content,
            None => {
                // Fetch robots.txt
                let robots_url = format!("{origin}/robots.txt");
                let content = match self.client.get(&robots_url).send().await {
                    Ok(resp) if resp.status().is_success() => resp.text().await.ok(),
                    _ => None, // 404, timeout, etc. → allow all
                };

                let mut cache = self.cache.lock().unwrap();
                cache.insert(origin, content.clone());
                content
            }
        };

        match robots_content {
            Some(content) => robotstxt::DefaultMatcher::default().one_agent_allowed_by_robots(
                &content,
                &self.user_agent,
                url,
            ),
            None => true, // No robots.txt or fetch failed → allow all
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_matcher_disallow() {
        let robots = "User-agent: *\nDisallow: /admin\n";
        let allowed = robotstxt::DefaultMatcher::default().one_agent_allowed_by_robots(
            robots,
            "Ares",
            "https://example.com/admin/page",
        );
        assert!(!allowed);
    }

    #[test]
    fn test_matcher_allow() {
        let robots = "User-agent: *\nDisallow: /admin\n";
        let allowed = robotstxt::DefaultMatcher::default().one_agent_allowed_by_robots(
            robots,
            "Ares",
            "https://example.com/public/page",
        );
        assert!(allowed);
    }

    #[test]
    fn test_matcher_specific_agent() {
        let robots = "User-agent: Ares\nDisallow: /secret\n\nUser-agent: *\nAllow: /\n";
        let allowed = robotstxt::DefaultMatcher::default().one_agent_allowed_by_robots(
            robots,
            "Ares",
            "https://example.com/secret",
        );
        assert!(!allowed);

        let allowed_other = robotstxt::DefaultMatcher::default().one_agent_allowed_by_robots(
            robots,
            "OtherBot",
            "https://example.com/secret",
        );
        assert!(allowed_other);
    }
}
