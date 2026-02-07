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
        }
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
        // 1. Fetch
        tracing::info!("Fetching {}", url);
        let html = self.fetcher.fetch(url).await?;
        tracing::info!("Fetched {} bytes of HTML", html.len());

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

        // 3. Extract
        tracing::info!("Extracting with model {} ...", self.model_name);
        let extracted = self.extractor.extract(&markdown, schema).await?;

        // 4. Hash
        let content_hash = compute_hash(&markdown);
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
        } else {
            (true, None)
        };

        Ok(ScrapeResult {
            extracted_data: extracted,
            content_hash,
            data_hash,
            changed,
            extraction_id,
        })
    }
}
