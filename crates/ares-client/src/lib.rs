//! HTTP clients and adapters — fetchers, HTML cleaner, and LLM extractor.

pub mod cleaner;
pub mod fetcher;
pub mod link_discovery;
pub mod llm;
pub mod robots;

#[cfg(feature = "browser")]
pub mod browser_fetcher;

pub use cleaner::HtmdCleaner;
pub use fetcher::ReqwestFetcher;
pub use link_discovery::HtmlLinkDiscoverer;
pub use llm::{OpenAiExtractor, OpenAiExtractorFactory};
pub use robots::CachedRobotsChecker;

#[cfg(feature = "browser")]
pub use browser_fetcher::BrowserFetcher;
