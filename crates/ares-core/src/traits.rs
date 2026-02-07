use std::future::Future;

use uuid::Uuid;

use crate::error::AppError;
use crate::models::{Extraction, NewExtraction};

/// Fetches raw HTML content from a URL.
pub trait Fetcher: Send + Sync + Clone {
    fn fetch(&self, url: &str) -> impl Future<Output = Result<String, AppError>> + Send;
}

/// Converts raw HTML into clean Markdown text.
pub trait Cleaner: Send + Sync {
    fn clean(&self, html: &str) -> Result<String, AppError>;
}

/// Extracts structured JSON data from text content using an LLM.
pub trait Extractor: Send + Sync + Clone {
    /// Sends the content and JSON schema to the LLM and returns extracted JSON.
    fn extract(
        &self,
        content: &str,
        schema: &serde_json::Value,
    ) -> impl Future<Output = Result<serde_json::Value, AppError>> + Send;
}

/// Persists and retrieves extraction results.
pub trait ExtractionStore: Send + Sync + Clone {
    /// Save a new extraction result. Returns the generated UUID.
    fn save(
        &self,
        extraction: &NewExtraction,
    ) -> impl Future<Output = Result<Uuid, AppError>> + Send;

    /// Get the most recent extraction for a URL + schema pair.
    fn get_latest(
        &self,
        url: &str,
        schema_name: &str,
    ) -> impl Future<Output = Result<Option<Extraction>, AppError>> + Send;

    /// Get extraction history for a URL + schema pair, newest first.
    fn get_history(
        &self,
        url: &str,
        schema_name: &str,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<Extraction>, AppError>> + Send;
}
