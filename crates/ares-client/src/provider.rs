//! Provider selection between OpenAI-compatible and native Anthropic backends.
//!
//! Both [`OpenAiExtractor`] and [`AnthropicExtractor`] implement [`Extractor`],
//! but they are distinct concrete types, so call sites that are generic over a
//! single extractor type can't switch between them at runtime. The dispatch
//! enums here ([`ProviderExtractor`] / [`ProviderExtractorFactory`]) wrap either
//! backend behind one type, letting the CLI and API pick a provider from
//! `ARES_PROVIDER` / `--provider` without duplicating the pipeline wiring.

use std::time::Duration;

use ares_core::error::AppError;
use ares_core::models::ExtractionOutcome;
use ares_core::traits::{Extractor, ExtractorFactory};

#[cfg(not(feature = "local-llm"))]
use crate::LOCAL_LLM_FEATURE_MSG;
use crate::llm::{OpenAiExtractor, OpenAiExtractorFactory};

#[cfg(feature = "anthropic")]
use crate::anthropic::{AnthropicExtractor, AnthropicExtractorFactory};

#[cfg(feature = "local-llm")]
use crate::candle::{CandleExtractor, CandleExtractorFactory};

#[cfg(not(feature = "anthropic"))]
const ANTHROPIC_FEATURE_MSG: &str = "Anthropic provider requires the `anthropic` feature. Rebuild with: cargo build --features anthropic";

/// Which LLM backend to use for extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Provider {
    /// OpenAI-compatible Chat Completions API (OpenAI, Gemini, local servers).
    #[default]
    OpenAi,
    /// Native Anthropic Messages API (Claude).
    Anthropic,
    /// Native CPU inference through Candle.
    Local,
}

impl Provider {
    /// Parse a provider name. Empty/`"openai"` → OpenAI; `"anthropic"`/`"claude"`
    /// → Anthropic. Anything else is a configuration error.
    pub fn parse(s: &str) -> Result<Self, AppError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "openai" => Ok(Provider::OpenAi),
            "anthropic" | "claude" => Ok(Provider::Anthropic),
            "local" | "candle" => Ok(Provider::Local),
            other => Err(AppError::ConfigError(format!(
                "Unknown provider '{other}'. Expected 'openai', 'anthropic', or 'local'."
            ))),
        }
    }

    /// The canonical lowercase name of this provider, as recorded in extraction
    /// run metadata and accepted by [`Provider::parse`].
    pub fn name(&self) -> &'static str {
        match self {
            Provider::OpenAi => "openai",
            Provider::Anthropic => "anthropic",
            Provider::Local => "local",
        }
    }

    /// The default API base URL for this provider (used when none is supplied).
    pub fn default_base_url(&self) -> &'static str {
        match self {
            Provider::OpenAi => "https://api.openai.com/v1",
            Provider::Anthropic => "https://api.anthropic.com/v1",
            Provider::Local => "local://",
        }
    }
}

/// An [`Extractor`] backed by whichever provider was selected.
#[derive(Clone)]
pub enum ProviderExtractor {
    OpenAi(OpenAiExtractor),
    #[cfg(feature = "anthropic")]
    Anthropic(AnthropicExtractor),
    #[cfg(feature = "local-llm")]
    Local(CandleExtractor),
}

impl ProviderExtractor {
    /// Build a one-shot extractor for the given provider.
    pub fn build(
        provider: Provider,
        api_key: &str,
        model: &str,
        base_url: &str,
        llm_timeout: Option<Duration>,
        system_prompt: Option<&str>,
    ) -> Result<Self, AppError> {
        match provider {
            Provider::OpenAi => {
                let mut e = OpenAiExtractor::with_base_url(api_key, model, base_url)?;
                if let Some(t) = llm_timeout {
                    e = e.with_timeout(t)?;
                }
                if let Some(p) = system_prompt {
                    e = e.with_system_prompt(p);
                }
                Ok(ProviderExtractor::OpenAi(e))
            }
            Provider::Anthropic => {
                #[cfg(feature = "anthropic")]
                {
                    let mut e = AnthropicExtractor::with_base_url(api_key, model, base_url)?;
                    if let Some(t) = llm_timeout {
                        e = e.with_timeout(t)?;
                    }
                    if let Some(p) = system_prompt {
                        e = e.with_system_prompt(p);
                    }
                    Ok(ProviderExtractor::Anthropic(e))
                }
                #[cfg(not(feature = "anthropic"))]
                {
                    let _ = (api_key, model, base_url, llm_timeout, system_prompt);
                    Err(AppError::ConfigError(ANTHROPIC_FEATURE_MSG.to_string()))
                }
            }
            Provider::Local => {
                #[cfg(feature = "local-llm")]
                {
                    let _ = (api_key, base_url, llm_timeout);
                    let mut e = CandleExtractor::new(model)?;
                    if let Some(p) = system_prompt {
                        e = e.with_system_prompt(p);
                    }
                    Ok(ProviderExtractor::Local(e))
                }
                #[cfg(not(feature = "local-llm"))]
                {
                    let _ = (api_key, model, base_url, llm_timeout, system_prompt);
                    Err(AppError::ConfigError(LOCAL_LLM_FEATURE_MSG.to_string()))
                }
            }
        }
    }
}

impl ProviderExtractor {
    /// The provider name for this extractor, for recording in run metadata.
    pub fn provider_name(&self) -> &'static str {
        match self {
            ProviderExtractor::OpenAi(_) => "openai",
            #[cfg(feature = "anthropic")]
            ProviderExtractor::Anthropic(_) => "anthropic",
            #[cfg(feature = "local-llm")]
            ProviderExtractor::Local(_) => "local",
        }
    }
}

impl Extractor for ProviderExtractor {
    async fn extract(
        &self,
        content: &str,
        schema: &serde_json::Value,
    ) -> Result<ExtractionOutcome, AppError> {
        match self {
            ProviderExtractor::OpenAi(e) => e.extract(content, schema).await,
            #[cfg(feature = "anthropic")]
            ProviderExtractor::Anthropic(e) => e.extract(content, schema).await,
            #[cfg(feature = "local-llm")]
            ProviderExtractor::Local(e) => e.extract(content, schema).await,
        }
    }
}

/// An [`ExtractorFactory`] backed by whichever provider was selected. Used by
/// the worker, which builds a per-job extractor from each job's model/base_url.
#[derive(Clone)]
pub enum ProviderExtractorFactory {
    OpenAi(OpenAiExtractorFactory),
    #[cfg(feature = "anthropic")]
    Anthropic(AnthropicExtractorFactory),
    #[cfg(feature = "local-llm")]
    Local(CandleExtractorFactory),
}

impl ProviderExtractorFactory {
    pub fn build(
        provider: Provider,
        api_key: &str,
        llm_timeout: Option<Duration>,
        system_prompt: Option<&str>,
    ) -> Result<Self, AppError> {
        match provider {
            Provider::OpenAi => {
                let mut f = OpenAiExtractorFactory::new(api_key);
                if let Some(t) = llm_timeout {
                    f = f.with_llm_timeout(t);
                }
                if let Some(p) = system_prompt {
                    f = f.with_system_prompt(p);
                }
                Ok(ProviderExtractorFactory::OpenAi(f))
            }
            Provider::Anthropic => {
                #[cfg(feature = "anthropic")]
                {
                    let mut f = AnthropicExtractorFactory::new(api_key);
                    if let Some(t) = llm_timeout {
                        f = f.with_llm_timeout(t);
                    }
                    if let Some(p) = system_prompt {
                        f = f.with_system_prompt(p);
                    }
                    Ok(ProviderExtractorFactory::Anthropic(f))
                }
                #[cfg(not(feature = "anthropic"))]
                {
                    let _ = (api_key, llm_timeout, system_prompt);
                    Err(AppError::ConfigError(ANTHROPIC_FEATURE_MSG.to_string()))
                }
            }
            Provider::Local => {
                #[cfg(feature = "local-llm")]
                {
                    let _ = (api_key, llm_timeout);
                    let mut f = CandleExtractorFactory::new()?;
                    if let Some(p) = system_prompt {
                        f = f.with_system_prompt(p);
                    }
                    Ok(ProviderExtractorFactory::Local(f))
                }
                #[cfg(not(feature = "local-llm"))]
                {
                    let _ = (api_key, llm_timeout, system_prompt);
                    Err(AppError::ConfigError(LOCAL_LLM_FEATURE_MSG.to_string()))
                }
            }
        }
    }
}

impl ExtractorFactory for ProviderExtractorFactory {
    type Extractor = ProviderExtractor;

    fn create(&self, model: &str, base_url: &str) -> Result<ProviderExtractor, AppError> {
        match self {
            ProviderExtractorFactory::OpenAi(f) => {
                Ok(ProviderExtractor::OpenAi(f.create(model, base_url)?))
            }
            #[cfg(feature = "anthropic")]
            ProviderExtractorFactory::Anthropic(f) => {
                Ok(ProviderExtractor::Anthropic(f.create(model, base_url)?))
            }
            #[cfg(feature = "local-llm")]
            ProviderExtractorFactory::Local(f) => {
                Ok(ProviderExtractor::Local(f.create(model, base_url)?))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_provider_names() {
        assert_eq!(Provider::parse("openai").unwrap(), Provider::OpenAi);
        assert_eq!(Provider::parse("OpenAI").unwrap(), Provider::OpenAi);
        assert_eq!(Provider::parse("").unwrap(), Provider::OpenAi);
        assert_eq!(Provider::parse("anthropic").unwrap(), Provider::Anthropic);
        assert_eq!(Provider::parse(" Claude ").unwrap(), Provider::Anthropic);
        assert_eq!(Provider::parse("local").unwrap(), Provider::Local);
        assert!(Provider::parse("gemini").is_err());
    }

    #[test]
    fn default_base_urls() {
        assert_eq!(
            Provider::OpenAi.default_base_url(),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            Provider::Anthropic.default_base_url(),
            "https://api.anthropic.com/v1"
        );
        assert_eq!(Provider::Local.default_base_url(), "local://");
    }

    #[test]
    fn openai_extractor_builds_without_feature() {
        let e = ProviderExtractor::build(
            Provider::OpenAi,
            "key",
            "gpt-4o-mini",
            "https://api.openai.com/v1",
            None,
            None,
        );
        assert!(e.is_ok());
    }

    #[cfg(not(feature = "anthropic"))]
    #[test]
    fn anthropic_errors_without_feature() {
        let e = ProviderExtractor::build(
            Provider::Anthropic,
            "key",
            "claude-haiku-4-5",
            "https://api.anthropic.com/v1",
            None,
            None,
        );
        assert!(matches!(e, Err(AppError::ConfigError(_))));
    }

    #[cfg(not(feature = "local-llm"))]
    #[test]
    fn local_errors_without_feature() {
        let e = ProviderExtractor::build(
            Provider::Local,
            "",
            "qwen2.5-3b-instruct-q4",
            "local://",
            None,
            None,
        );
        assert!(matches!(e, Err(AppError::ConfigError(_))));
    }

    #[cfg(feature = "anthropic")]
    #[test]
    fn anthropic_builds_with_feature() {
        let e = ProviderExtractor::build(
            Provider::Anthropic,
            "key",
            "claude-haiku-4-5",
            "https://api.anthropic.com/v1",
            None,
            None,
        );
        assert!(matches!(e, Ok(ProviderExtractor::Anthropic(_))));
    }
}
