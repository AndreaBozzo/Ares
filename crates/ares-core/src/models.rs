use std::sync::Arc;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// User-defined extraction schema (JSON Schema subset).
///
/// The `schema` field is the raw JSON Schema passed to the LLM
/// to instruct structured output.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExtractionSchema {
    /// Human-readable schema name (e.g., "real_estate_listing")
    pub name: String,
    /// JSON Schema definition for the LLM
    pub schema: serde_json::Value,
}

/// Token usage reported by an LLM for a single extraction call.
///
/// Native/local backends (Candle) have no billable-token notion, so they report
/// `None` usage on [`ExtractionOutcome`]; hosted providers populate this from
/// their API response. Used as a cost proxy and recorded as run metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

impl Usage {
    pub fn new(prompt_tokens: u32, completion_tokens: u32) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
        }
    }

    /// Total tokens billed for the call (prompt + completion).
    pub fn total_tokens(&self) -> u32 {
        self.prompt_tokens.saturating_add(self.completion_tokens)
    }
}

/// The result of an [`Extractor::extract`](crate::traits::Extractor::extract)
/// call: the extracted JSON value plus optional token usage.
///
/// Usage is `Option` because local backends can't report billable tokens.
/// Latency is measured by the pipeline (around the call), not carried here.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExtractionOutcome {
    pub value: serde_json::Value,
    pub usage: Option<Usage>,
}

impl ExtractionOutcome {
    /// An outcome with no usage information (local backends, mocks).
    pub fn new(value: serde_json::Value) -> Self {
        Self { value, usage: None }
    }

    /// An outcome carrying reported token usage.
    pub fn with_usage(value: serde_json::Value, usage: Usage) -> Self {
        Self {
            value,
            usage: Some(usage),
        }
    }
}

impl From<serde_json::Value> for ExtractionOutcome {
    fn from(value: serde_json::Value) -> Self {
        Self::new(value)
    }
}

/// A completed extraction result.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Extraction {
    pub id: Uuid,
    pub url: String,
    pub schema_name: String,
    pub extracted_data: serde_json::Value,
    /// SHA-256 of the cleaned markdown content
    pub content_hash: String,
    /// SHA-256 of the extracted JSON data (for change detection)
    pub data_hash: String,
    pub model: String,
    pub created_at: DateTime<Utc>,
}

/// DTO for inserting a new extraction into the database.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NewExtraction {
    pub url: String,
    pub schema_name: String,
    pub extracted_data: serde_json::Value,
    pub raw_content_hash: String,
    pub data_hash: String,
    pub model: String,
}

/// Result of a scrape pipeline execution.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScrapeResult {
    /// The extracted structured data.
    pub extracted_data: serde_json::Value,
    /// SHA-256 of the cleaned markdown content.
    pub content_hash: String,
    /// SHA-256 of the extracted JSON data.
    pub data_hash: String,
    /// Whether data changed compared to the previous extraction.
    pub changed: bool,
    /// The persisted extraction ID (if saved to DB).
    pub extraction_id: Option<Uuid>,
    /// Wall-clock time spent in the extractor call (LLM round-trip), in ms.
    /// `None` when the result was served from the extraction cache.
    pub latency_ms: Option<u128>,
    /// Token usage reported by the extractor. `None` for local backends or
    /// cache hits.
    pub usage: Option<Usage>,
    /// The raw HTML content (used for link discovery in crawling).
    #[serde(skip)]
    pub raw_html: Option<Arc<str>>,
}

/// Compute a SHA-256 hash of a string, returned as 64-char hex.
pub fn compute_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_hash_consistency() {
        let h1 = compute_hash("hello world");
        let h2 = compute_hash("hello world");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn test_compute_hash_different_inputs() {
        let h1 = compute_hash("hello");
        let h2 = compute_hash("world");
        assert_ne!(h1, h2);
    }
}
