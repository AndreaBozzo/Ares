use serde::{Deserialize, Serialize};

/// Configuration for a crawl session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlConfig {
    /// Maximum depth from the seed URL.
    pub max_depth: u32,

    /// Maximum number of pages to crawl in a session.
    pub max_pages: u32,

    /// Optional list of allowed domains (defaults to seed domain if empty).
    pub allowed_domains: Vec<String>,

    /// Respect robots.txt rules.
    pub respect_robots_txt: bool,

    /// Optional regex pattern for URLs to follow.
    pub url_pattern: Option<String>,
}

impl Default for CrawlConfig {
    fn default() -> Self {
        Self {
            max_depth: 3,
            max_pages: 100,
            allowed_domains: Vec::new(),
            respect_robots_txt: true,
            url_pattern: None,
        }
    }
}

impl CrawlConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_depth(mut self, depth: u32) -> Self {
        self.max_depth = depth;
        self
    }

    pub fn with_max_pages(mut self, pages: u32) -> Self {
        self.max_pages = pages;
        self
    }

    pub fn with_allowed_domains(mut self, domains: Vec<String>) -> Self {
        self.allowed_domains = domains;
        self
    }
}
