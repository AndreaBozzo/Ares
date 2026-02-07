use ares_core::error::AppError;
use ares_core::traits::Extractor;
use reqwest::Client;
use serde::{Deserialize, Serialize};

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// OpenAI-compatible LLM client for structured extraction.
///
/// Works with any OpenAI-compatible API, including:
/// - OpenAI directly (`https://api.openai.com/v1`)
/// - Gemini via compatibility layer (`https://generativelanguage.googleapis.com/v1beta/openai`)
#[derive(Clone)]
pub struct OpenAiExtractor {
    client: Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl OpenAiExtractor {
    pub fn new(api_key: &str, model: &str) -> Result<Self, AppError> {
        Self::with_base_url(api_key, model, DEFAULT_BASE_URL)
    }

    pub fn with_base_url(api_key: &str, model: &str, base_url: &str) -> Result<Self, AppError> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| AppError::HttpError(e.to_string()))?;

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
        })
    }
}

// ---- OpenAI API types ----

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
}

#[derive(Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    format_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    json_schema: Option<JsonSchemaWrapper>,
}

#[derive(Serialize)]
struct JsonSchemaWrapper {
    name: String,
    strict: bool,
    schema: serde_json::Value,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct ApiError {
    error: ApiErrorDetail,
}

#[derive(Deserialize)]
struct ApiErrorDetail {
    message: String,
}

impl Extractor for OpenAiExtractor {
    async fn extract(
        &self,
        content: &str,
        schema: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let url = format!("{}/chat/completions", self.base_url);

        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                Message {
                    role: "system".to_string(),
                    content: "You are a data extraction assistant. Extract the requested fields from the provided web content. Respond ONLY with valid JSON matching the requested schema. Do not include explanations.".to_string(),
                },
                Message {
                    role: "user".to_string(),
                    content: format!(
                        "Extract data according to this JSON schema:\n```json\n{}\n```\n\nFrom the following web content:\n\n{}",
                        serde_json::to_string_pretty(schema).unwrap_or_default(),
                        content
                    ),
                },
            ],
            response_format: Some(ResponseFormat {
                format_type: "json_schema".to_string(),
                json_schema: Some(JsonSchemaWrapper {
                    name: "extraction".to_string(),
                    strict: true,
                    schema: schema.clone(),
                }),
            }),
        };

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    AppError::Timeout(120)
                } else if e.is_connect() {
                    AppError::NetworkError(format!("Connection failed: {}", e))
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
                .unwrap_or_else(|_| format!("HTTP {}: {}", status_code, body));

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

        let chat_response: ChatResponse = response
            .json()
            .await
            .map_err(|e| AppError::HttpError(format!("Failed to parse LLM response: {}", e)))?;

        let content_str = chat_response
            .choices
            .first()
            .and_then(|c| c.message.content.as_ref())
            .ok_or_else(|| AppError::LlmError {
                message: "Empty response from LLM".into(),
                status_code: 200,
                retryable: false,
            })?;

        serde_json::from_str(content_str).map_err(|e| {
            AppError::SchemaValidationError(format!(
                "LLM returned invalid JSON: {}. Raw: {}",
                e, content_str
            ))
        })
    }
}
