use std::sync::Arc;

use ares_core::error::AppError;
use ares_core::traits::Cleaner;
use htmd::HtmlToMarkdown;
use scraper::{Html, Selector};

/// HTML-to-Markdown cleaner using htmd.
///
/// Converts raw HTML into clean Markdown text, stripping non-content
/// elements (script, style, nav, etc.) to minimize LLM token usage.
///
/// htmd drops `<head>` and skips `<header>`/`<footer>`, so page metadata that
/// lives only in `<head>` (canonical URL, Open Graph image, author/date meta)
/// would never reach the extractor — and a model asked for those fields tends to
/// *hallucinate* plausible values rather than omit them. To prevent that, a
/// small "Page metadata" block harvested from `<head>` is prepended to the
/// Markdown so those fields are grounded in real input.
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
        let body = self
            .converter
            .convert(html)
            .map_err(|e| AppError::CleanerError(e.to_string()))?;

        let metadata = extract_metadata(html);
        if metadata.is_empty() {
            return Ok(body);
        }

        let mut out = String::from("## Page metadata\n");
        for (label, value) in metadata {
            out.push_str(&format!("- {label}: {value}\n"));
        }
        out.push_str("\n---\n\n");
        out.push_str(&body);
        Ok(out)
    }
}

/// Harvest grounded metadata from `<head>` (and `<title>`) as `(label, value)`
/// pairs. Only fields actually present are returned, first match wins.
fn extract_metadata(html: &str) -> Vec<(&'static str, String)> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();

    // Each entry: label → ordered list of (css selector, attribute) sources.
    // `__text__` means take the element's text instead of an attribute.
    let fields: &[(&str, &[(&str, &str)])] = &[
        (
            "URL",
            &[
                ("link[rel=canonical]", "href"),
                ("meta[property='og:url']", "content"),
            ],
        ),
        (
            "Title",
            &[
                ("meta[property='og:title']", "content"),
                ("title", "__text__"),
            ],
        ),
        (
            "Author",
            &[
                ("meta[name=author]", "content"),
                ("meta[property='article:author']", "content"),
            ],
        ),
        (
            "Published",
            &[
                ("meta[property='article:published_time']", "content"),
                ("meta[name=date]", "content"),
                ("meta[name='publish_date']", "content"),
            ],
        ),
        (
            "Image",
            &[
                ("meta[property='og:image']", "content"),
                ("meta[name='twitter:image']", "content"),
            ],
        ),
        (
            "Description",
            &[
                ("meta[name=description]", "content"),
                ("meta[property='og:description']", "content"),
            ],
        ),
    ];

    for (label, sources) in fields {
        if let Some(value) = first_value(&doc, sources) {
            out.push((*label, value));
        }
    }
    out
}

/// Return the first non-empty value across the given `(selector, attr)` sources.
fn first_value(doc: &Html, sources: &[(&str, &str)]) -> Option<String> {
    for (selector, attr) in sources {
        let Ok(sel) = Selector::parse(selector) else {
            continue;
        };
        if let Some(el) = doc.select(&sel).next() {
            let raw = if *attr == "__text__" {
                el.text().collect::<String>()
            } else {
                el.value().attr(attr).unwrap_or_default().to_string()
            };
            let trimmed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    None
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

    #[test]
    fn test_strips_style_tags() {
        let cleaner = HtmdCleaner::new();
        let html = "<p>Visible</p><style>body { color: red; }</style>";
        let md = cleaner.clean(html).unwrap();
        assert!(md.contains("Visible"));
        assert!(!md.contains("color"));
    }

    #[test]
    fn test_strips_nav_and_footer() {
        let cleaner = HtmdCleaner::new();
        let html = "<nav><a href='/'>Home</a></nav><main><p>Article</p></main><footer>Copyright 2025</footer>";
        let md = cleaner.clean(html).unwrap();
        assert!(md.contains("Article"));
        assert!(!md.contains("Copyright"));
    }

    #[test]
    fn test_strips_noscript_iframe_svg() {
        let cleaner = HtmdCleaner::new();
        let html = concat!(
            "<p>Main</p>",
            "<noscript>Enable JS</noscript>",
            "<iframe src='ad.html'>Ad</iframe>",
            "<svg><circle r='10'/></svg>",
        );
        let md = cleaner.clean(html).unwrap();
        assert!(md.contains("Main"));
        assert!(!md.contains("Enable JS"));
        assert!(!md.contains("Ad"));
        assert!(!md.contains("circle"));
    }

    #[test]
    fn test_preserves_content_elements() {
        let cleaner = HtmdCleaner::new();
        let html = concat!(
            "<article><h2>Title</h2></article>",
            "<section><p>Section text</p></section>",
            "<div>Div content</div>",
            "<table><tr><td>Cell</td></tr></table>",
        );
        let md = cleaner.clean(html).unwrap();
        assert!(md.contains("Title"));
        assert!(md.contains("Section text"));
        assert!(md.contains("Div content"));
        assert!(md.contains("Cell"));
    }

    #[test]
    fn test_no_metadata_block_without_head() {
        // Fragments with no <head>/meta must be unchanged (no preamble).
        let cleaner = HtmdCleaner::new();
        let md = cleaner.clean("<p>Body only</p>").unwrap();
        assert!(!md.contains("Page metadata"));
        assert!(md.starts_with("Body only"));
    }

    #[test]
    fn test_prepends_head_metadata() {
        let cleaner = HtmdCleaner::new();
        let html = concat!(
            "<html><head>",
            "<title>My Post — Site</title>",
            "<link rel=\"canonical\" href=\"https://ex.com/posts/my-post\">",
            "<meta property=\"og:image\" content=\"https://ex.com/img/hero.png\">",
            "<meta name=\"author\" content=\"Jane Doe\">",
            "<meta property=\"article:published_time\" content=\"2026-05-14\">",
            "<meta name=\"description\" content=\"A short summary.\">",
            "</head><body><p>The article body.</p></body></html>",
        );
        let md = cleaner.clean(html).unwrap();

        assert!(md.contains("## Page metadata"));
        assert!(md.contains("URL: https://ex.com/posts/my-post"));
        assert!(md.contains("Image: https://ex.com/img/hero.png"));
        assert!(md.contains("Author: Jane Doe"));
        assert!(md.contains("Published: 2026-05-14"));
        assert!(md.contains("Description: A short summary."));
        // Body still present, after the metadata block.
        assert!(md.contains("The article body."));
        let meta_idx = md.find("Page metadata").unwrap();
        let body_idx = md.find("The article body.").unwrap();
        assert!(meta_idx < body_idx);
    }

    #[test]
    fn test_og_fallbacks_when_no_canonical() {
        let cleaner = HtmdCleaner::new();
        let html = concat!(
            "<html><head>",
            "<meta property=\"og:url\" content=\"https://ex.com/p\">",
            "<meta property=\"og:title\" content=\"OG Title\">",
            "</head><body><p>x</p></body></html>",
        );
        let md = cleaner.clean(html).unwrap();
        assert!(md.contains("URL: https://ex.com/p"));
        assert!(md.contains("Title: OG Title"));
    }
}
