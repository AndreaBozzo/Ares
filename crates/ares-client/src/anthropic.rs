//! Anthropic (Claude) extractor using the native Messages API.
//!
//! Anthropic's API is **not** OpenAI-compatible: it uses the Messages endpoint
//! (`/v1/messages`), `x-api-key` auth, and an `anthropic-version` header.
//! Structured extraction is done via **forced tool use** — we declare a single
//! `extract` tool whose `input_schema` is the caller's JSON Schema and force
//! Claude to call it (`tool_choice: {"type": "tool", ...}`). Claude then returns
//! a `tool_use` block whose `input` is the structured result. The output is
//! still validated against the schema by `ScrapeService`, so we deliberately do
//! not set `strict: true` (which would reject arbitrary user schemas that lack
//! `additionalProperties: false`).
//!
//! Feature-gated behind `anthropic` so OpenAI-only builds don't compile it.

use std::time::Duration;

use ares_core::error::AppError;
use ares_core::traits::{Extractor, ExtractorFactory};
use reqwest::Client;
use serde::{Deserialize, Serialize};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_LLM_TIMEOUT: Duration = Duration::from_secs(120);
/// Required by the Messages API. Extraction output is small, but lists can be
/// sizeable — keep generous headroom to avoid truncated (invalid) JSON.
const DEFAULT_MAX_TOKENS: u32 = 8192;
const TOOL_NAME: &str = "extract";
const DEFAULT_SYSTEM_PROMPT: &str = "You are a data extraction assistant. Extract the requested fields from the provided web content by calling the `extract` tool with arguments matching the requested schema. Do not include explanations.";

/// Anthropic Messages API client for structured extraction with Claude models.
///
/// Recommended models: `claude-haiku-4-5` (fast, cheap, high-volume) and
/// `claude-sonnet-4-6` (complex schemas, higher quality).
#[derive(Clone)]
pub struct AnthropicExtractor {
    client: Client,
    base_url: String,
    api_key: String,
    model: String,
    timeout_secs: u64,
    max_tokens: u32,
    system_prompt: String,
}

impl AnthropicExtractor {
    pub fn new(api_key: &str, model: &str) -> Result<Self, AppError> {
        Self::with_base_url(api_key, model, DEFAULT_BASE_URL)
    }

    pub fn with_base_url(api_key: &str, model: &str, base_url: &str) -> Result<Self, AppError> {
        Self::build(
            api_key,
            model,
            base_url,
            DEFAULT_LLM_TIMEOUT,
            DEFAULT_MAX_TOKENS,
        )
    }

    pub fn with_timeout(self, timeout: Duration) -> Result<Self, AppError> {
        Self::build(
            &self.api_key,
            &self.model,
            &self.base_url,
            timeout,
            self.max_tokens,
        )
        .map(|e| e.with_system_prompt(self.system_prompt))
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    fn build(
        api_key: &str,
        model: &str,
        base_url: &str,
        timeout: Duration,
        max_tokens: u32,
    ) -> Result<Self, AppError> {
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| AppError::HttpError(e.to_string()))?;

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
            timeout_secs: timeout.as_secs(),
            max_tokens,
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
        })
    }

    fn build_request(&self, content: &str, schema: &serde_json::Value) -> MessagesRequest {
        MessagesRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            system: self.system_prompt.clone(),
            tools: vec![Tool {
                name: TOOL_NAME.to_string(),
                description: "Return the structured data extracted from the web content."
                    .to_string(),
                input_schema: schema.clone(),
            }],
            tool_choice: ToolChoice {
                choice_type: "tool".to_string(),
                name: TOOL_NAME.to_string(),
            },
            messages: vec![Message {
                role: "user".to_string(),
                content: format!(
                    "Extract data matching the `extract` tool's schema from the following web content:\n\n{content}"
                ),
            }],
        }
    }
}

// ---- Anthropic Messages API types ----

#[derive(Serialize)]
struct MessagesRequest {
    model: String,
    max_tokens: u32,
    system: String,
    tools: Vec<Tool>,
    tool_choice: ToolChoice,
    messages: Vec<Message>,
}

#[derive(Serialize)]
struct Tool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Serialize)]
struct ToolChoice {
    #[serde(rename = "type")]
    choice_type: String,
    name: String,
}

#[derive(Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct MessagesResponse {
    #[serde(default)]
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    input: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ApiError {
    error: ApiErrorDetail,
}

#[derive(Deserialize)]
struct ApiErrorDetail {
    message: String,
}

/// Extract the forced-tool result from a Messages API response body.
///
/// Pure function (no HTTP) so it can be unit-tested against recorded responses.
fn parse_extraction(body: &str) -> Result<serde_json::Value, AppError> {
    let response: MessagesResponse = serde_json::from_str(body).map_err(|e| {
        AppError::HttpError(format!(
            "Failed to parse Anthropic response: {e}. Raw: {body}"
        ))
    })?;

    response
        .content
        .into_iter()
        .find(|b| b.block_type == "tool_use")
        .and_then(|b| b.input)
        .ok_or_else(|| AppError::LlmError {
            message: format!("No tool_use block in Anthropic response. Raw: {body}"),
            status_code: 200,
            retryable: false,
        })
}

impl Extractor for AnthropicExtractor {
    async fn extract(
        &self,
        content: &str,
        schema: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let url = format!("{}/messages", self.base_url);
        let request = self.build_request(content, schema);

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    AppError::Timeout(self.timeout_secs)
                } else if e.is_connect() {
                    AppError::NetworkError(format!("Connection failed: {e}"))
                } else {
                    AppError::HttpError(e.to_string())
                }
            })?;

        let status = response.status();
        if !status.is_success() {
            let status_code = status.as_u16();
            let body = response.text().await.unwrap_or_default();

            let message = serde_json::from_str::<ApiError>(&body)
                .map(|e| e.error.message)
                .unwrap_or_else(|_| format!("HTTP {status_code}: {body}"));

            // Anthropic uses 429 (rate limit), 500 (api_error), 529 (overloaded).
            let retryable = status_code == 429 || status_code >= 500;

            if status_code == 429 {
                return Err(AppError::RateLimitExceeded);
            }

            return Err(AppError::LlmError {
                message,
                status_code,
                retryable,
            });
        }

        let body = response
            .text()
            .await
            .map_err(|e| AppError::HttpError(format!("Failed to read Anthropic response: {e}")))?;

        parse_extraction(&body)
    }
}

/// Factory that creates `AnthropicExtractor` instances with a shared API key.
///
/// Used by the worker to construct per-job extractors, since each job may
/// specify a different model or base URL.
#[derive(Clone)]
pub struct AnthropicExtractorFactory {
    api_key: String,
    llm_timeout: Option<Duration>,
    max_tokens: Option<u32>,
    system_prompt: Option<String>,
}

impl AnthropicExtractorFactory {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            llm_timeout: None,
            max_tokens: None,
            system_prompt: None,
        }
    }

    pub fn with_llm_timeout(mut self, timeout: Duration) -> Self {
        self.llm_timeout = Some(timeout);
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }
}

impl ExtractorFactory for AnthropicExtractorFactory {
    type Extractor = AnthropicExtractor;

    fn create(&self, model: &str, base_url: &str) -> Result<AnthropicExtractor, AppError> {
        let mut extractor = AnthropicExtractor::with_base_url(&self.api_key, model, base_url)?;
        if let Some(m) = self.max_tokens {
            extractor = extractor.with_max_tokens(m);
        }
        if let Some(t) = self.llm_timeout {
            extractor = extractor.with_timeout(t)?;
        }
        if let Some(p) = &self.system_prompt {
            extractor = extractor.with_system_prompt(p.clone());
        }
        Ok(extractor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "title": { "type": "string" } }
        })
    }

    #[test]
    fn build_request_forces_tool_and_embeds_schema() {
        let extractor = AnthropicExtractor::new("key", "claude-haiku-4-5").unwrap();
        let req = extractor.build_request("hello world", &schema());

        assert_eq!(req.model, "claude-haiku-4-5");
        assert_eq!(req.max_tokens, DEFAULT_MAX_TOKENS);
        assert_eq!(req.tool_choice.choice_type, "tool");
        assert_eq!(req.tool_choice.name, TOOL_NAME);
        assert_eq!(req.tools.len(), 1);
        assert_eq!(req.tools[0].name, TOOL_NAME);
        assert_eq!(req.tools[0].input_schema, schema());
        assert!(req.messages[0].content.contains("hello world"));

        // Serializes with the wire field name `type` for tool_choice.
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["tool_choice"]["type"], "tool");
        assert_eq!(json["system"], DEFAULT_SYSTEM_PROMPT);
    }

    #[test]
    fn parse_extraction_returns_tool_input() {
        let body = serde_json::json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [
                { "type": "tool_use", "id": "toolu_1", "name": "extract", "input": { "title": "Hello" } }
            ],
            "stop_reason": "tool_use"
        })
        .to_string();

        let value = parse_extraction(&body).unwrap();
        assert_eq!(value, serde_json::json!({ "title": "Hello" }));
    }

    #[test]
    fn parse_extraction_skips_leading_text_block() {
        // Claude may emit a text block before the tool_use block.
        let body = serde_json::json!({
            "content": [
                { "type": "text", "text": "Let me extract that." },
                { "type": "tool_use", "name": "extract", "input": { "title": "Hi" } }
            ]
        })
        .to_string();

        assert_eq!(
            parse_extraction(&body).unwrap(),
            serde_json::json!({ "title": "Hi" })
        );
    }

    #[test]
    fn parse_extraction_errors_when_no_tool_use() {
        let body = serde_json::json!({
            "content": [ { "type": "text", "text": "I cannot help with that." } ],
            "stop_reason": "end_turn"
        })
        .to_string();

        let err = parse_extraction(&body).unwrap_err();
        assert!(matches!(err, AppError::LlmError { .. }));
    }

    #[test]
    fn parse_extraction_errors_on_malformed_json() {
        let err = parse_extraction("not json").unwrap_err();
        assert!(matches!(err, AppError::HttpError(_)));
    }

    #[test]
    fn factory_creates_extractor_with_model() {
        let factory = AnthropicExtractorFactory::new("key").with_max_tokens(4096);
        let extractor = factory
            .create("claude-sonnet-4-6", "https://api.anthropic.com/v1")
            .unwrap();
        assert_eq!(extractor.model, "claude-sonnet-4-6");
        assert_eq!(extractor.max_tokens, 4096);
    }
}
