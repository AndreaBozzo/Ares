use std::future::Future;

use crate::error::AppError;

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
