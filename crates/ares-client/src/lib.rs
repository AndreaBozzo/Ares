//! HTTP clients and adapters — fetchers, HTML cleaner, and LLM extractor.

pub mod cleaner;
pub mod fetcher;
pub mod link_discovery;
pub mod llm;
pub mod provider;
pub mod robots;
pub mod user_agent;
pub(crate) mod util;

#[cfg(feature = "local-llm")]
pub mod candle;

#[cfg(feature = "anthropic")]
pub mod anthropic;

#[cfg(feature = "browser")]
pub mod browser_fetcher;

pub use cleaner::HtmdCleaner;
pub use fetcher::ReqwestFetcher;
pub use link_discovery::HtmlLinkDiscoverer;
pub use llm::{OpenAiExtractor, OpenAiExtractorFactory};
pub use provider::{Provider, ProviderExtractor, ProviderExtractorFactory};
pub use robots::CachedRobotsChecker;
pub use user_agent::UserAgentPool;

/// The only native model alias supported by the first local-inference release.
pub const LOCAL_MODEL_ALIAS: &str = "qwen2.5-3b-instruct-q4";

/// Explains how to enable the optional native inference backend.
pub const LOCAL_LLM_FEATURE_MSG: &str = "Local provider requires the `local-llm` feature. Rebuild with: cargo build --features local-llm";

#[cfg(feature = "anthropic")]
pub use anthropic::{AnthropicExtractor, AnthropicExtractorFactory};

#[cfg(feature = "local-llm")]
pub use candle::{CandleExtractor, CandleExtractorFactory, LocalModelStatus, LocalModelStore};

#[cfg(feature = "browser")]
pub use browser_fetcher::BrowserFetcher;
