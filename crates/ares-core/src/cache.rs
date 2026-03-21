use std::time::Duration;

use moka::future::Cache;

use crate::models::compute_hash;

/// Configuration for in-memory caches.
#[derive(Debug, Clone)]
pub struct CacheConfig {
    pub ttl: Duration,
    pub max_content_entries: u64,
    pub max_extraction_entries: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(3600),
            max_content_entries: 1_000,
            max_extraction_entries: 10_000,
        }
    }
}

/// Cache for fetched HTML content. Keyed by URL hash.
#[derive(Clone)]
pub struct ContentCache {
    inner: Cache<String, String>,
}

impl ContentCache {
    pub fn new(config: &CacheConfig) -> Self {
        Self {
            inner: Cache::builder()
                .max_capacity(config.max_content_entries)
                .time_to_live(config.ttl)
                .build(),
        }
    }

    pub async fn get(&self, url: &str) -> Option<String> {
        let key = compute_hash(url);
        let result = self.inner.get(&key).await;
        if result.is_some() {
            tracing::debug!(url, "Content cache HIT");
        } else {
            tracing::debug!(url, "Content cache MISS");
        }
        result
    }

    pub async fn insert(&self, url: &str, html: String) {
        let key = compute_hash(url);
        self.inner.insert(key, html).await;
    }
}

/// Cache for LLM extraction results. Keyed by content_hash + schema_name + model.
#[derive(Clone)]
pub struct ExtractionCache {
    inner: Cache<String, serde_json::Value>,
}

impl ExtractionCache {
    pub fn new(config: &CacheConfig) -> Self {
        Self {
            inner: Cache::builder()
                .max_capacity(config.max_extraction_entries)
                .time_to_live(config.ttl)
                .build(),
        }
    }

    fn cache_key(content_hash: &str, schema_name: &str, schema_hash: &str, model: &str) -> String {
        compute_hash(&format!(
            "{content_hash}:{schema_name}:{schema_hash}:{model}"
        ))
    }

    pub async fn get(
        &self,
        content_hash: &str,
        schema_name: &str,
        schema_hash: &str,
        model: &str,
    ) -> Option<serde_json::Value> {
        let key = Self::cache_key(content_hash, schema_name, schema_hash, model);
        let result = self.inner.get(&key).await;
        if result.is_some() {
            tracing::debug!(schema_name, model, "Extraction cache HIT");
        } else {
            tracing::debug!(schema_name, model, "Extraction cache MISS");
        }
        result
    }

    pub async fn insert(
        &self,
        content_hash: &str,
        schema_name: &str,
        schema_hash: &str,
        model: &str,
        data: serde_json::Value,
    ) {
        let key = Self::cache_key(content_hash, schema_name, schema_hash, model);
        self.inner.insert(key, data).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> CacheConfig {
        CacheConfig {
            ttl: Duration::from_secs(60),
            max_content_entries: 100,
            max_extraction_entries: 100,
        }
    }

    #[tokio::test]
    async fn content_cache_miss_then_hit() {
        let cache = ContentCache::new(&test_config());

        assert!(cache.get("https://example.com").await.is_none());

        cache
            .insert("https://example.com", "<html>hello</html>".into())
            .await;

        let cached = cache.get("https://example.com").await;
        assert_eq!(cached.unwrap(), "<html>hello</html>");
    }

    #[tokio::test]
    async fn content_cache_different_urls() {
        let cache = ContentCache::new(&test_config());

        cache.insert("https://a.com", "page A".into()).await;
        cache.insert("https://b.com", "page B".into()).await;

        assert_eq!(cache.get("https://a.com").await.unwrap(), "page A");
        assert_eq!(cache.get("https://b.com").await.unwrap(), "page B");
        assert!(cache.get("https://c.com").await.is_none());
    }

    #[tokio::test]
    async fn extraction_cache_miss_then_hit() {
        let cache = ExtractionCache::new(&test_config());
        let data = serde_json::json!({"title": "Hello"});

        assert!(
            cache
                .get("hash1", "articles", "sh1", "gpt-4o")
                .await
                .is_none()
        );

        cache
            .insert("hash1", "articles", "sh1", "gpt-4o", data.clone())
            .await;

        let cached = cache.get("hash1", "articles", "sh1", "gpt-4o").await;
        assert_eq!(cached.unwrap(), data);
    }

    #[tokio::test]
    async fn extraction_cache_key_differs_by_model() {
        let cache = ExtractionCache::new(&test_config());
        let data_a = serde_json::json!({"title": "A"});
        let data_b = serde_json::json!({"title": "B"});

        cache
            .insert("hash1", "articles", "sh1", "gpt-4o", data_a.clone())
            .await;
        cache
            .insert(
                "hash1",
                "articles",
                "sh1",
                "gemini-2.5-flash",
                data_b.clone(),
            )
            .await;

        assert_eq!(
            cache
                .get("hash1", "articles", "sh1", "gpt-4o")
                .await
                .unwrap(),
            data_a
        );
        assert_eq!(
            cache
                .get("hash1", "articles", "sh1", "gemini-2.5-flash")
                .await
                .unwrap(),
            data_b
        );
    }

    #[tokio::test]
    async fn extraction_cache_key_differs_by_schema() {
        let cache = ExtractionCache::new(&test_config());
        let data_a = serde_json::json!({"title": "A"});
        let data_b = serde_json::json!({"price": 42});

        cache
            .insert("hash1", "articles", "sh_a", "gpt-4o", data_a.clone())
            .await;
        cache
            .insert("hash1", "products", "sh_b", "gpt-4o", data_b.clone())
            .await;

        assert_eq!(
            cache
                .get("hash1", "articles", "sh_a", "gpt-4o")
                .await
                .unwrap(),
            data_a
        );
        assert_eq!(
            cache
                .get("hash1", "products", "sh_b", "gpt-4o")
                .await
                .unwrap(),
            data_b
        );
    }

    #[test]
    fn cache_config_default_values() {
        let config = CacheConfig::default();
        assert_eq!(config.ttl, Duration::from_secs(3600));
        assert_eq!(config.max_content_entries, 1_000);
        assert_eq!(config.max_extraction_entries, 10_000);
    }
}
