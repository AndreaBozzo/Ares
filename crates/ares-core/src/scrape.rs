use std::sync::Arc;

use crate::cache::{ContentCache, ExtractionCache};
use crate::error::AppError;
use crate::models::{NewExtraction, ScrapeResult, compute_hash};
use crate::traits::{Cleaner, ExtractionStore, Extractor, Fetcher};

/// Orchestrates the full scrape pipeline: fetch → clean → extract → hash → compare → save.
///
/// Generic over all external dependencies via traits, enabling dependency injection
/// and testability without real HTTP or LLM calls.
pub struct ScrapeService<F, C, E, S>
where
    F: Fetcher,
    C: Cleaner,
    E: Extractor,
    S: ExtractionStore,
{
    fetcher: F,
    cleaner: C,
    extractor: E,
    store: Option<S>,
    model_name: String,
    skip_unchanged: bool,
    content_cache: Option<ContentCache>,
    extraction_cache: Option<ExtractionCache>,
}

impl<F, C, E, S> ScrapeService<F, C, E, S>
where
    F: Fetcher,
    C: Cleaner,
    E: Extractor,
    S: ExtractionStore,
{
    /// Create a new ScrapeService without persistence.
    pub fn new(fetcher: F, cleaner: C, extractor: E, model_name: String) -> Self {
        Self {
            fetcher,
            cleaner,
            extractor,
            store: None,
            model_name,
            skip_unchanged: false,
            content_cache: None,
            extraction_cache: None,
        }
    }

    /// Create a new ScrapeService with database persistence.
    pub fn with_store(fetcher: F, cleaner: C, extractor: E, store: S, model_name: String) -> Self {
        Self {
            fetcher,
            cleaner,
            extractor,
            store: Some(store),
            model_name,
            skip_unchanged: false,
            content_cache: None,
            extraction_cache: None,
        }
    }

    /// When enabled, skip saving if the data hash matches the previous extraction.
    pub fn with_skip_unchanged(mut self, skip: bool) -> Self {
        self.skip_unchanged = skip;
        self
    }

    /// Enable in-memory caching for fetched content and LLM extraction results.
    pub fn with_caches(
        mut self,
        content: Option<ContentCache>,
        extraction: Option<ExtractionCache>,
    ) -> Self {
        self.content_cache = content;
        self.extraction_cache = extraction;
        self
    }

    /// Run the full scrape pipeline for a URL + schema.
    ///
    /// 1. Fetch HTML from URL
    /// 2. Clean HTML to Markdown
    /// 3. Extract structured data via LLM
    /// 4. Compute content and data hashes
    /// 5. Compare with previous extraction (if store available)
    /// 6. Persist result (if store available)
    pub async fn scrape(
        &self,
        url: &str,
        schema: &serde_json::Value,
        schema_name: &str,
    ) -> Result<ScrapeResult, AppError> {
        // 1. Fetch (with optional content cache)
        let html: Arc<str> = if let Some(cache) = &self.content_cache {
            if let Some(cached) = cache.get(url).await {
                tracing::info!("Using cached content for {} ({} bytes)", url, cached.len());
                cached
            } else {
                tracing::info!("Fetching {}", url);
                let html: Arc<str> = self.fetcher.fetch(url).await?.into();
                tracing::info!("Fetched {} bytes of HTML", html.len());
                cache.insert(url, Arc::clone(&html)).await;
                html
            }
        } else {
            tracing::info!("Fetching {}", url);
            let html: Arc<str> = self.fetcher.fetch(url).await?.into();
            tracing::info!("Fetched {} bytes of HTML", html.len());
            html
        };

        // 2. Clean
        let markdown = self.cleaner.clean(&html)?;
        tracing::info!(
            "Cleaned to {} bytes of Markdown ({}% reduction)",
            markdown.len(),
            if html.is_empty() {
                0
            } else {
                100 - (markdown.len() * 100 / html.len())
            }
        );

        // 3. Hash content and schema (before extraction, needed for extraction cache key)
        let content_hash = compute_hash(&markdown);
        let schema_hash = compute_hash(&schema.to_string());

        // 4. Extract (with optional extraction cache)
        let extracted = if let Some(cache) = &self.extraction_cache {
            if let Some(cached) = cache
                .get(&content_hash, schema_name, &schema_hash, &self.model_name)
                .await
            {
                tracing::info!("Using cached extraction for model {}", self.model_name);
                cached
            } else {
                tracing::info!("Extracting with model {} ...", self.model_name);
                let data = self.extractor.extract(&markdown, schema).await?;
                cache
                    .insert(
                        &content_hash,
                        schema_name,
                        &schema_hash,
                        &self.model_name,
                        data.clone(),
                    )
                    .await;
                data
            }
        } else {
            tracing::info!("Extracting with model {} ...", self.model_name);
            self.extractor.extract(&markdown, schema).await?
        };

        // 5. Hash extracted data
        let data_hash = compute_hash(&extracted.to_string());
        tracing::info!(
            content_hash = %&content_hash[..8],
            data_hash = %&data_hash[..8],
            "Extraction complete"
        );

        // 5 & 6. Compare + Persist
        let (changed, extraction_id) = if let Some(store) = &self.store {
            let previous = store.get_latest(url, schema_name).await?;
            let changed = match &previous {
                Some(prev) => prev.data_hash != data_hash,
                None => true,
            };

            if self.skip_unchanged && !changed {
                let prev_id = previous.map(|p| p.id);
                tracing::info!(?prev_id, "Data unchanged — skipping save");
                (false, prev_id)
            } else {
                let new_extraction = NewExtraction {
                    url: url.to_string(),
                    schema_name: schema_name.to_string(),
                    extracted_data: extracted.clone(),
                    raw_content_hash: content_hash.clone(),
                    data_hash: data_hash.clone(),
                    model: self.model_name.clone(),
                };

                let id = store.save(&new_extraction).await?;

                if changed {
                    if previous.is_some() {
                        tracing::info!(%id, "Data CHANGED — saved new extraction");
                    } else {
                        tracing::info!(%id, "First extraction — saved");
                    }
                } else {
                    tracing::info!(%id, "Data unchanged — saved snapshot");
                }

                (changed, Some(id))
            }
        } else {
            (true, None)
        };

        Ok(ScrapeResult {
            extracted_data: extracted,
            content_hash,
            data_hash,
            changed,
            extraction_id,
            raw_html: Some(html),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::compute_hash;
    use crate::testutil::*;
    use crate::traits::NullStore;

    fn test_schema() -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"title": {"type": "string"}}})
    }

    #[tokio::test]
    async fn happy_path_without_store() {
        let extracted = serde_json::json!({"title": "Hello"});
        let svc = ScrapeService::<_, _, _, NullStore>::new(
            MockFetcher::new("<html>hello</html>"),
            MockCleaner::passthrough(),
            MockExtractor::new(extracted.clone()),
            "test-model".into(),
        );

        let result = svc
            .scrape("https://example.com", &test_schema(), "test")
            .await
            .unwrap();

        assert_eq!(result.extracted_data, extracted);
        assert!(result.changed);
        assert!(result.extraction_id.is_none());
        assert_eq!(result.content_hash.len(), 64);
        assert_eq!(result.data_hash.len(), 64);
    }

    #[tokio::test]
    async fn happy_path_with_store_first_extraction() {
        let extracted = serde_json::json!({"title": "Hello"});
        let store = MockStore::empty();
        let svc = ScrapeService::with_store(
            MockFetcher::new("<html>hello</html>"),
            MockCleaner::passthrough(),
            MockExtractor::new(extracted.clone()),
            store.clone(),
            "test-model".into(),
        );

        let result = svc
            .scrape("https://example.com", &test_schema(), "test")
            .await
            .unwrap();

        assert!(result.changed);
        assert!(result.extraction_id.is_some());
        assert_eq!(store.saved.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn with_store_same_data_hash_reports_unchanged() {
        let extracted = serde_json::json!({"title": "Hello"});
        let data_hash = compute_hash(&extracted.to_string());
        let prev = make_test_extraction(&data_hash);
        let store = MockStore::with_latest(prev);

        let svc = ScrapeService::with_store(
            MockFetcher::new("<html>hello</html>"),
            MockCleaner::passthrough(),
            MockExtractor::new(extracted),
            store.clone(),
            "test-model".into(),
        );

        let result = svc
            .scrape("https://example.com", &test_schema(), "test")
            .await
            .unwrap();

        assert!(!result.changed);
        assert!(result.extraction_id.is_some());
        // Still saves the extraction (snapshot)
        assert_eq!(store.saved.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn with_store_different_data_hash_reports_changed() {
        let prev = make_test_extraction("old_hash_that_wont_match");
        let store = MockStore::with_latest(prev);

        let svc = ScrapeService::with_store(
            MockFetcher::new("<html>hello</html>"),
            MockCleaner::passthrough(),
            MockExtractor::new(serde_json::json!({"title": "New Title"})),
            store,
            "test-model".into(),
        );

        let result = svc
            .scrape("https://example.com", &test_schema(), "test")
            .await
            .unwrap();

        assert!(result.changed);
    }

    #[tokio::test]
    async fn fetch_error_propagates() {
        let svc = ScrapeService::<_, _, _, NullStore>::new(
            MockFetcher::with_error(AppError::HttpError("connection refused".into())),
            MockCleaner::passthrough(),
            MockExtractor::new(serde_json::json!({})),
            "test-model".into(),
        );

        let err = svc
            .scrape("https://example.com", &test_schema(), "test")
            .await
            .unwrap_err();

        assert!(matches!(err, AppError::HttpError(_)));
    }

    #[tokio::test]
    async fn clean_error_propagates() {
        let svc = ScrapeService::<_, _, _, NullStore>::new(
            MockFetcher::new("<html>hello</html>"),
            MockCleaner::with_error(AppError::CleanerError("bad html".into())),
            MockExtractor::new(serde_json::json!({})),
            "test-model".into(),
        );

        let err = svc
            .scrape("https://example.com", &test_schema(), "test")
            .await
            .unwrap_err();

        assert!(matches!(err, AppError::CleanerError(_)));
    }

    #[tokio::test]
    async fn extract_error_propagates() {
        let svc = ScrapeService::<_, _, _, NullStore>::new(
            MockFetcher::new("<html>hello</html>"),
            MockCleaner::passthrough(),
            MockExtractor::with_error(AppError::LlmError {
                message: "overloaded".into(),
                status_code: 503,
                retryable: true,
            }),
            "test-model".into(),
        );

        let err = svc
            .scrape("https://example.com", &test_schema(), "test")
            .await
            .unwrap_err();

        assert!(matches!(err, AppError::LlmError { .. }));
    }

    #[tokio::test]
    async fn skip_unchanged_skips_save() {
        let extracted = serde_json::json!({"title": "Hello"});
        let data_hash = compute_hash(&extracted.to_string());
        let prev = make_test_extraction(&data_hash);
        let prev_id = prev.id;
        let store = MockStore::with_latest(prev);

        let svc = ScrapeService::with_store(
            MockFetcher::new("<html>hello</html>"),
            MockCleaner::passthrough(),
            MockExtractor::new(extracted),
            store.clone(),
            "test-model".into(),
        )
        .with_skip_unchanged(true);

        let result = svc
            .scrape("https://example.com", &test_schema(), "test")
            .await
            .unwrap();

        assert!(!result.changed);
        assert_eq!(result.extraction_id, Some(prev_id));
        assert_eq!(store.saved.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn skip_unchanged_false_still_saves_snapshot() {
        let extracted = serde_json::json!({"title": "Hello"});
        let data_hash = compute_hash(&extracted.to_string());
        let prev = make_test_extraction(&data_hash);
        let store = MockStore::with_latest(prev);

        let svc = ScrapeService::with_store(
            MockFetcher::new("<html>hello</html>"),
            MockCleaner::passthrough(),
            MockExtractor::new(extracted),
            store.clone(),
            "test-model".into(),
        )
        .with_skip_unchanged(false);

        let result = svc
            .scrape("https://example.com", &test_schema(), "test")
            .await
            .unwrap();

        assert!(!result.changed);
        assert!(result.extraction_id.is_some());
        assert_eq!(store.saved.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn store_save_error_propagates() {
        let store = MockStore::with_save_error(AppError::DatabaseError("disk full".into()));

        let svc = ScrapeService::with_store(
            MockFetcher::new("<html>hello</html>"),
            MockCleaner::passthrough(),
            MockExtractor::new(serde_json::json!({"title": "Test"})),
            store,
            "test-model".into(),
        );

        let err = svc
            .scrape("https://example.com", &test_schema(), "test")
            .await
            .unwrap_err();

        assert!(matches!(err, AppError::DatabaseError(_)));
    }

    // -----------------------------------------------------------------------
    // Cache integration tests
    // -----------------------------------------------------------------------

    fn test_cache_config() -> crate::cache::CacheConfig {
        crate::cache::CacheConfig {
            ttl: std::time::Duration::from_secs(60),
            max_content_entries: 100,
            max_extraction_entries: 100,
        }
    }

    #[tokio::test]
    async fn content_cache_avoids_second_fetch() {
        let config = test_cache_config();
        let content_cache = crate::cache::ContentCache::new(&config);
        let extraction_cache = crate::cache::ExtractionCache::new(&config);

        // MockFetcher with only ONE response — second call would return default
        let extracted = serde_json::json!({"title": "Hello"});
        let svc = ScrapeService::<_, _, _, NullStore>::new(
            MockFetcher::new("<html>hello</html>"),
            MockCleaner::passthrough(),
            MockExtractor::with_responses(vec![Ok(extracted.clone()), Ok(extracted.clone())]),
            "test-model".into(),
        )
        .with_caches(Some(content_cache), Some(extraction_cache));

        // First scrape — fetches from MockFetcher
        let r1 = svc
            .scrape("https://example.com", &test_schema(), "test")
            .await
            .unwrap();
        assert_eq!(r1.extracted_data, extracted);

        // Second scrape — should use content cache (MockFetcher has no more responses)
        let r2 = svc
            .scrape("https://example.com", &test_schema(), "test")
            .await
            .unwrap();
        assert_eq!(r2.extracted_data, extracted);
        // Same content means same content hash
        assert_eq!(r1.content_hash, r2.content_hash);
    }

    #[tokio::test]
    async fn extraction_cache_avoids_second_llm_call() {
        let config = test_cache_config();
        let content_cache = crate::cache::ContentCache::new(&config);
        let extraction_cache = crate::cache::ExtractionCache::new(&config);

        // MockExtractor with only ONE response — second call would return default
        let extracted = serde_json::json!({"title": "Hello"});
        let svc = ScrapeService::<_, _, _, NullStore>::new(
            MockFetcher::with_responses(vec![
                Ok("<html>hello</html>".into()),
                Ok("<html>hello</html>".into()),
            ]),
            MockCleaner::passthrough(),
            MockExtractor::new(extracted.clone()),
            "test-model".into(),
        )
        .with_caches(Some(content_cache), Some(extraction_cache));

        // First scrape
        let r1 = svc
            .scrape("https://a.com", &test_schema(), "test")
            .await
            .unwrap();
        assert_eq!(r1.extracted_data, extracted);

        // Second scrape — different URL but same content after cleaning.
        // Extraction cache should hit (same content_hash + schema + model).
        let r2 = svc
            .scrape("https://b.com", &test_schema(), "test")
            .await
            .unwrap();
        assert_eq!(r2.extracted_data, extracted);
    }

    #[tokio::test]
    async fn no_cache_calls_fetcher_every_time() {
        let extracted = serde_json::json!({"title": "Hello"});
        let svc = ScrapeService::<_, _, _, NullStore>::new(
            MockFetcher::with_responses(vec![
                Ok("<html>first</html>".into()),
                Ok("<html>second</html>".into()),
            ]),
            MockCleaner::passthrough(),
            MockExtractor::with_responses(vec![
                Ok(extracted.clone()),
                Ok(serde_json::json!({"title": "World"})),
            ]),
            "test-model".into(),
        );
        // No caches (default None)

        let r1 = svc
            .scrape("https://example.com", &test_schema(), "test")
            .await
            .unwrap();
        assert_eq!(r1.extracted_data, extracted);

        let r2 = svc
            .scrape("https://example.com", &test_schema(), "test")
            .await
            .unwrap();
        // Different extraction because no cache — fetcher returned different HTML
        assert_eq!(r2.extracted_data, serde_json::json!({"title": "World"}));
    }
}
