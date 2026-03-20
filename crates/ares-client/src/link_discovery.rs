use scraper::{Html, Selector};
use url::Url;

use ares_core::error::AppError;
use ares_core::traits::LinkDiscoverer;

/// Link discoverer that uses the `scraper` crate for HTML parsing.
#[derive(Debug, Clone, Default)]
pub struct HtmlLinkDiscoverer;

impl HtmlLinkDiscoverer {
    pub fn new() -> Self {
        Self
    }
}

impl LinkDiscoverer for HtmlLinkDiscoverer {
    fn discover_links(&self, html: &str, base_url: &str) -> Result<Vec<String>, AppError> {
        let document = Html::parse_document(html);
        let selector = Selector::parse("a[href]").map_err(|e| {
            AppError::Generic(format!("Failed to parse CSS selector for links: {}", e))
        })?;

        let base = Url::parse(base_url)
            .map_err(|e| AppError::Generic(format!("Invalid base URL '{}': {}", base_url, e)))?;

        let mut links = Vec::new();

        for element in document.select(&selector) {
            if let Some(href) = element.value().attr("href") {
                // Resolve relative URL
                match base.join(href) {
                    Ok(full_url) => {
                        // Only include http/https links
                        if full_url.scheme() == "http" || full_url.scheme() == "https" {
                            // Strip fragment
                            let mut normalized = full_url;
                            normalized.set_fragment(None);
                            links.push(normalized.to_string());
                        }
                    }
                    Err(e) => {
                        tracing::debug!(href, error = %e, "Failed to resolve link");
                    }
                }
            }
        }

        // Deduplicate
        links.sort();
        links.dedup();

        Ok(links)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discover_links_basic() {
        let html = r##"
            <html>
                <body>
                    <a href="/page1">Page 1</a>
                    <a href="https://example.com/page2">Page 2</a>
                    <a href="mailto:test@example.com">Email</a>
                    <a href="#fragment">Fragment</a>
                    <a href="/page1?q=1">Page 1 with query</a>
                </body>
            </html>
        "##;
        let discoverer = HtmlLinkDiscoverer::new();
        let links = discoverer
            .discover_links(html, "https://example.com")
            .unwrap();

        assert_eq!(links.len(), 3);
        assert!(links.contains(&"https://example.com/page1".to_string()));
        assert!(links.contains(&"https://example.com/page2".to_string()));
        assert!(links.contains(&"https://example.com/page1?q=1".to_string()));
    }

    #[test]
    fn test_discover_links_relative_base() {
        let html = r#"<a href="sub">Link</a>"#;
        let discoverer = HtmlLinkDiscoverer::new();
        let links = discoverer
            .discover_links(html, "https://example.com/blog/")
            .unwrap();

        assert_eq!(links.len(), 1);
        assert_eq!(links[0], "https://example.com/blog/sub");
    }

    #[test]
    fn test_discover_links_normalization() {
        let html = r##"
            <a href="/page#1">1</a>
            <a href="/page#2">2</a>
        "##;
        let discoverer = HtmlLinkDiscoverer::new();
        let links = discoverer
            .discover_links(html, "https://example.com")
            .unwrap();

        assert_eq!(links.len(), 1);
        assert_eq!(links[0], "https://example.com/page");
    }
}
