use std::sync::Arc;

use ares_core::error::AppError;
use ares_core::traits::Cleaner;
use htmd::HtmlToMarkdown;

/// HTML-to-Markdown cleaner using htmd.
///
/// Converts raw HTML into clean Markdown text, stripping non-content
/// elements (script, style, nav, etc.) to minimize LLM token usage.
pub struct HtmdCleaner {
    converter: Arc<HtmlToMarkdown>,
}

impl Clone for HtmdCleaner {
    fn clone(&self) -> Self {
        Self {
            converter: Arc::clone(&self.converter),
        }
    }
}

impl HtmdCleaner {
    pub fn new() -> Self {
        let converter = HtmlToMarkdown::builder()
            .skip_tags(vec![
                "script", "style", "nav", "footer", "header", "aside", "noscript", "iframe", "svg",
            ])
            .build();

        Self {
            converter: Arc::new(converter),
        }
    }
}

impl Default for HtmdCleaner {
    fn default() -> Self {
        Self::new()
    }
}

impl Cleaner for HtmdCleaner {
    fn clean(&self, html: &str) -> Result<String, AppError> {
        self.converter
            .convert(html)
            .map_err(|e| AppError::CleanerError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_html_to_markdown() {
        let cleaner = HtmdCleaner::new();
        let html = "<h1>Hello</h1><p>World</p>";
        let md = cleaner.clean(html).unwrap();
        assert!(md.contains("Hello"));
        assert!(md.contains("World"));
    }

    #[test]
    fn test_strips_script_tags() {
        let cleaner = HtmdCleaner::new();
        let html = "<p>Content</p><script>alert('xss')</script>";
        let md = cleaner.clean(html).unwrap();
        assert!(md.contains("Content"));
        assert!(!md.contains("alert"));
    }
}
