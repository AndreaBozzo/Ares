//! Core library for Ares — traits, pipeline logic, job scheduling, and error types.

pub mod circuit_breaker;
pub mod error;
pub mod job;
pub mod job_queue;
pub mod models;
pub mod schema;
pub mod scrape;
pub mod throttle;
pub mod traits;
pub mod worker;

#[cfg(test)]
pub mod testutil;

pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
pub use error::AppError;
pub use job::{CreateScrapeJobRequest, JobStatus, RetryConfig, ScrapeJob, WorkerConfig};
pub use job_queue::JobQueue;
pub use models::{Extraction, ExtractionSchema, NewExtraction, ScrapeResult, compute_hash};
pub use schema::{ResolvedSchema, SchemaEntry, SchemaResolver, derive_schema_name};
pub use scrape::ScrapeService;
pub use throttle::{ThrottleConfig, ThrottledFetcher};
pub use traits::{Cleaner, ExtractionStore, Extractor, ExtractorFactory, Fetcher, NullStore};
pub use worker::{WorkerEvent, WorkerService};
