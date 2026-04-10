//! Core library for Ares — traits, pipeline logic, job scheduling, and error types.

pub mod cache;
pub mod circuit_breaker;
pub mod crawl;
pub mod error;
pub mod job;
pub mod job_queue;
pub mod models;
pub mod proxy;
pub mod rand;
pub mod schema;
pub mod scrape;
pub mod stealth;
pub mod throttle;
pub mod traits;
pub mod worker;

#[cfg(test)]
pub mod testutil;

pub use cache::{CacheConfig, ContentCache, ExtractionCache};
pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
pub use crawl::CrawlConfig;
pub use error::AppError;
pub use job::{CreateScrapeJobRequest, JobStatus, RetryConfig, ScrapeJob, WorkerConfig};
pub use job_queue::JobQueue;
pub use models::{Extraction, ExtractionSchema, NewExtraction, ScrapeResult, compute_hash};
pub use proxy::{ProxyConfig, ProxyEntry, RotationStrategy, TlsBackend};
pub use schema::{
    ResolvedSchema, SchemaEntry, SchemaResolver, derive_schema_name, validate_schema,
};
pub use scrape::ScrapeService;
pub use stealth::StealthConfig;
pub use throttle::{ThrottleConfig, ThrottledFetcher};
pub use traits::{
    Cleaner, ExtractionStore, Extractor, ExtractorFactory, Fetcher, LinkDiscoverer,
    NoRobotsChecker, NullStore, RobotsChecker,
};
pub use worker::{WorkerEvent, WorkerService};
