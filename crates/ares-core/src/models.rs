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
