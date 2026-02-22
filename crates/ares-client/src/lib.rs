//! HTTP clients and adapters — fetchers, HTML cleaner, and LLM extractor.

pub mod cleaner;
pub mod fetcher;
pub mod llm;

#[cfg(feature = "browser")]
pub mod browser_fetcher;

pub use cleaner::HtmdCleaner;
pub use fetcher::ReqwestFetcher;
pub use llm::{OpenAiExtractor, OpenAiExtractorFactory};

#[cfg(feature = "browser")]
pub use browser_fetcher::BrowserFetcher;
