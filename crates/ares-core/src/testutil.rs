//! Test utilities: mock implementations of all core traits.
//!
//! Handwritten mocks for dependency injection in unit tests.
//! All mocks use `Arc<Mutex<_>>` for interior mutability, allowing
//! test assertions on recorded calls.

use std::sync::{Arc, Mutex};

use chrono::Utc;
use uuid::Uuid;

use crate::error::AppError;
use crate::job::{CreateScrapeJobRequest, JobStatus, ScrapeJob};
use crate::job_queue::JobQueue;
use crate::models::{Extraction, NewExtraction};
use crate::traits::{
    Cleaner, ExtractionStore, Extractor, ExtractorFactory, Fetcher, LinkDiscoverer,
};

// ---------------------------------------------------------------------------
// MockFetcher
// ---------------------------------------------------------------------------

/// Mock fetcher that returns a configurable response.
#[derive(Clone)]
pub struct MockFetcher {
    /// Queue of responses. Each call pops the first element.
    /// If empty, returns a default HTML string.
    responses: Arc<Mutex<Vec<Result<String, AppError>>>>,
}

impl MockFetcher {
    pub fn new(html: &str) -> Self {
        Self {
            responses: Arc::new(Mutex::new(vec![Ok(html.to_string())])),
        }
    }

    pub fn with_error(error: AppError) -> Self {
        Self {
            responses: Arc::new(Mutex::new(vec![Err(error)])),
        }
    }

    pub fn with_responses(responses: Vec<Result<String, AppError>>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses)),
        }
    }
}

impl Fetcher for MockFetcher {
    async fn fetch(&self, _url: &str) -> Result<String, AppError> {
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            Ok("<html><body>default</body></html>".to_string())
        } else {
            responses.remove(0)
        }
    }
}

// ---------------------------------------------------------------------------
// MockCleaner
// ---------------------------------------------------------------------------

/// Mock cleaner that applies a simple transformation.
#[derive(Clone)]
pub struct MockCleaner {
    error: Arc<Mutex<Option<AppError>>>,
}

impl MockCleaner {
    /// Creates a cleaner that returns the input unchanged.
    pub fn passthrough() -> Self {
        Self {
            error: Arc::new(Mutex::new(None)),
        }
    }

    /// Creates a cleaner that returns an error.
    pub fn with_error(error: AppError) -> Self {
        Self {
            error: Arc::new(Mutex::new(Some(error))),
        }
    }
}

impl Cleaner for MockCleaner {
    fn clean(&self, html: &str) -> Result<String, AppError> {
        let mut err = self.error.lock().unwrap();
        if let Some(e) = err.take() {
            return Err(e);
        }
        // Simulate cleaning: just return the input as-is
        Ok(html.to_string())
    }
}

// ---------------------------------------------------------------------------
// MockExtractor
// ---------------------------------------------------------------------------

/// Mock extractor that returns configurable JSON.
#[derive(Clone)]
pub struct MockExtractor {
    responses: Arc<Mutex<Vec<Result<serde_json::Value, AppError>>>>,
}

impl MockExtractor {
    pub fn new(data: serde_json::Value) -> Self {
        Self {
            responses: Arc::new(Mutex::new(vec![Ok(data)])),
        }
    }

    pub fn with_error(error: AppError) -> Self {
        Self {
            responses: Arc::new(Mutex::new(vec![Err(error)])),
        }
    }

    pub fn with_responses(responses: Vec<Result<serde_json::Value, AppError>>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses)),
        }
    }
}

impl Extractor for MockExtractor {
    async fn extract(
        &self,
        _content: &str,
        _schema: &serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            Ok(serde_json::json!({"default": true}))
        } else {
            responses.remove(0)
        }
    }
}

// ---------------------------------------------------------------------------
// MockExtractorFactory
// ---------------------------------------------------------------------------

/// Mock factory that always creates a MockExtractor with the given JSON response.
#[derive(Clone)]
pub struct MockExtractorFactory {
    /// The JSON value every created extractor will return.
    data: Arc<Mutex<serde_json::Value>>,
    create_error: Arc<Mutex<Option<AppError>>>,
}

impl MockExtractorFactory {
    pub fn new(data: serde_json::Value) -> Self {
        Self {
            data: Arc::new(Mutex::new(data)),
            create_error: Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_create_error(error: AppError) -> Self {
        Self {
            data: Arc::new(Mutex::new(serde_json::Value::Null)),
            create_error: Arc::new(Mutex::new(Some(error))),
        }
    }
}

impl ExtractorFactory for MockExtractorFactory {
    type Extractor = MockExtractor;

    fn create(&self, _model: &str, _base_url: &str) -> Result<MockExtractor, AppError> {
        let mut err = self.create_error.lock().unwrap();
        if let Some(e) = err.take() {
            return Err(e);
        }
        let data = self.data.lock().unwrap().clone();
        Ok(MockExtractor::new(data))
    }
}

// ---------------------------------------------------------------------------
// MockStore
// ---------------------------------------------------------------------------

/// Mock store that records saves and returns configurable latest/history.
#[derive(Clone)]
pub struct MockStore {
    pub saved: Arc<Mutex<Vec<NewExtraction>>>,
    latest: Arc<Mutex<Option<Extraction>>>,
    save_error: Arc<Mutex<Option<AppError>>>,
}

impl MockStore {
    /// Empty store — first extraction, no previous data.
    pub fn empty() -> Self {
        Self {
            saved: Arc::new(Mutex::new(Vec::new())),
            latest: Arc::new(Mutex::new(None)),
            save_error: Arc::new(Mutex::new(None)),
        }
    }

    /// Store with a previous extraction (for change detection tests).
    pub fn with_latest(extraction: Extraction) -> Self {
        Self {
            saved: Arc::new(Mutex::new(Vec::new())),
            latest: Arc::new(Mutex::new(Some(extraction))),
            save_error: Arc::new(Mutex::new(None)),
        }
    }

    /// Store that returns an error on save.
    pub fn with_save_error(error: AppError) -> Self {
        Self {
            saved: Arc::new(Mutex::new(Vec::new())),
            latest: Arc::new(Mutex::new(None)),
            save_error: Arc::new(Mutex::new(Some(error))),
        }
    }
}

impl ExtractionStore for MockStore {
    async fn save(&self, extraction: &NewExtraction) -> Result<Uuid, AppError> {
        let mut err = self.save_error.lock().unwrap();
        if let Some(e) = err.take() {
            return Err(e);
        }
        let id = Uuid::new_v4();
        self.saved.lock().unwrap().push(extraction.clone());
        Ok(id)
    }

    async fn get_latest(
        &self,
        _url: &str,
        _schema_name: &str,
    ) -> Result<Option<Extraction>, AppError> {
        Ok(self.latest.lock().unwrap().clone())
    }

    async fn get_history(
        &self,
        _url: &str,
        _schema_name: &str,
        _limit: usize,
        _offset: usize,
    ) -> Result<Vec<Extraction>, AppError> {
        Ok(vec![])
    }
}

// ---------------------------------------------------------------------------
// MockLinkDiscoverer
// ---------------------------------------------------------------------------

/// Mock link discoverer that returns a fixed set of links.
#[derive(Clone, Default)]
pub struct MockLinkDiscoverer {
    pub links: Arc<Mutex<Vec<String>>>,
}

impl MockLinkDiscoverer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_links(links: Vec<String>) -> Self {
        Self {
            links: Arc::new(Mutex::new(links)),
        }
    }
}

impl LinkDiscoverer for MockLinkDiscoverer {
    fn discover_links(&self, _html: &str, _base_url: &str) -> Result<Vec<String>, AppError> {
        Ok(self.links.lock().unwrap().clone())
    }
}

// ---------------------------------------------------------------------------
// MockRobotsChecker
// ---------------------------------------------------------------------------

/// Mock robots.txt checker that allows all URLs by default.
#[derive(Clone)]
pub struct MockRobotsChecker {
    pub blocked_urls: Arc<Mutex<Vec<String>>>,
}

impl MockRobotsChecker {
    pub fn new() -> Self {
        Self {
            blocked_urls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn with_blocked(urls: Vec<String>) -> Self {
        Self {
            blocked_urls: Arc::new(Mutex::new(urls)),
        }
    }
}

impl Default for MockRobotsChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::traits::RobotsChecker for MockRobotsChecker {
    async fn is_allowed(&self, url: &str) -> bool {
        let blocked = self.blocked_urls.lock().unwrap();
        !blocked.iter().any(|b| url.contains(b))
    }
}

// ---------------------------------------------------------------------------
// MockJobQueue
// ---------------------------------------------------------------------------

/// Recorded failure: (job_id, error_message, next_retry_at).
pub type FailedJobRecord = (Uuid, String, Option<chrono::DateTime<Utc>>);

/// Recorded completion: (job_id, extraction_id).
pub type CompletedJobRecord = (Uuid, Option<Uuid>);

/// Mock job queue backed by an in-memory Vec.
#[derive(Clone)]
pub struct MockJobQueue {
    pub jobs: Arc<Mutex<Vec<ScrapeJob>>>,
    claim_error: Arc<Mutex<Option<AppError>>>,
    pub failed_jobs: Arc<Mutex<Vec<FailedJobRecord>>>,
    pub completed_jobs: Arc<Mutex<Vec<CompletedJobRecord>>>,
    pub released_workers: Arc<Mutex<Vec<String>>>,
    pub visited_urls: Arc<Mutex<Vec<(Uuid, String)>>>,
}

impl MockJobQueue {
    pub fn empty() -> Self {
        Self {
            jobs: Arc::new(Mutex::new(Vec::new())),
            claim_error: Arc::new(Mutex::new(None)),
            failed_jobs: Arc::new(Mutex::new(Vec::new())),
            completed_jobs: Arc::new(Mutex::new(Vec::new())),
            released_workers: Arc::new(Mutex::new(Vec::new())),
            visited_urls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Queue with one pending job ready to be claimed.
    pub fn with_job(job: ScrapeJob) -> Self {
        Self {
            jobs: Arc::new(Mutex::new(vec![job])),
            claim_error: Arc::new(Mutex::new(None)),
            failed_jobs: Arc::new(Mutex::new(Vec::new())),
            completed_jobs: Arc::new(Mutex::new(Vec::new())),
            released_workers: Arc::new(Mutex::new(Vec::new())),
            visited_urls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn with_claim_error(error: AppError) -> Self {
        Self {
            jobs: Arc::new(Mutex::new(Vec::new())),
            claim_error: Arc::new(Mutex::new(Some(error))),
            failed_jobs: Arc::new(Mutex::new(Vec::new())),
            completed_jobs: Arc::new(Mutex::new(Vec::new())),
            released_workers: Arc::new(Mutex::new(Vec::new())),
            visited_urls: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl JobQueue for MockJobQueue {
    async fn create_job(&self, request: CreateScrapeJobRequest) -> Result<ScrapeJob, AppError> {
        let job = ScrapeJob {
            id: Uuid::new_v4(),
            url: request.url,
            schema_name: request.schema_name,
            schema: request.schema,
            model: request.model,
            base_url: request.base_url,
            status: JobStatus::Pending,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            started_at: None,
            completed_at: None,
            retry_count: 0,
            max_retries: request.max_retries.unwrap_or(3),
            next_retry_at: None,
            error_message: None,
            extraction_id: None,
            worker_id: None,
            crawl_session_id: request.crawl_session_id,
            parent_job_id: request.parent_job_id,
            depth: request.depth,
            max_depth: request.max_depth,
            max_pages: request.max_pages,
            allowed_domains: request.allowed_domains,
        };
        self.jobs.lock().unwrap().push(job.clone());
        Ok(job)
    }

    async fn claim_job(&self, worker_id: &str) -> Result<Option<ScrapeJob>, AppError> {
        let mut err = self.claim_error.lock().unwrap();
        if let Some(e) = err.take() {
            return Err(e);
        }

        let mut jobs = self.jobs.lock().unwrap();
        if let Some(pos) = jobs.iter().position(|j| j.status == JobStatus::Pending) {
            jobs[pos].status = JobStatus::Running;
            jobs[pos].worker_id = Some(worker_id.to_string());
            jobs[pos].started_at = Some(Utc::now());
            Ok(Some(jobs[pos].clone()))
        } else {
            Ok(None)
        }
    }

    async fn complete_job(
        &self,
        job_id: Uuid,
        extraction_id: Option<Uuid>,
    ) -> Result<(), AppError> {
        self.completed_jobs
            .lock()
            .unwrap()
            .push((job_id, extraction_id));

        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.iter_mut().find(|j| j.id == job_id) {
            job.status = JobStatus::Completed;
            job.extraction_id = extraction_id;
            job.completed_at = Some(Utc::now());
        }
        Ok(())
    }

    async fn fail_job(
        &self,
        job_id: Uuid,
        error: &str,
        next_retry_at: Option<chrono::DateTime<Utc>>,
    ) -> Result<(), AppError> {
        self.failed_jobs
            .lock()
            .unwrap()
            .push((job_id, error.to_string(), next_retry_at));

        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.iter_mut().find(|j| j.id == job_id) {
            if next_retry_at.is_some() {
                job.status = JobStatus::Pending;
                job.retry_count += 1;
                job.next_retry_at = next_retry_at;
            } else {
                job.status = JobStatus::Failed;
            }
            job.error_message = Some(error.to_string());
            job.worker_id = None;
        }
        Ok(())
    }

    async fn cancel_job(&self, job_id: Uuid) -> Result<(), AppError> {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.iter_mut().find(|j| j.id == job_id) {
            job.status = JobStatus::Cancelled;
        }
        Ok(())
    }

    async fn get_job(&self, job_id: Uuid) -> Result<Option<ScrapeJob>, AppError> {
        let jobs = self.jobs.lock().unwrap();
        Ok(jobs.iter().find(|j| j.id == job_id).cloned())
    }

    async fn list_jobs(
        &self,
        status: Option<JobStatus>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ScrapeJob>, AppError> {
        let jobs = self.jobs.lock().unwrap();
        let filtered: Vec<_> = jobs
            .iter()
            .filter(|j| status.is_none_or(|s| j.status == s))
            .skip(offset)
            .take(limit)
            .cloned()
            .collect();
        Ok(filtered)
    }

    async fn retry_job(&self, job_id: Uuid) -> Result<Option<ScrapeJob>, AppError> {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.iter_mut().find(|j| j.id == job_id)
            && matches!(job.status, JobStatus::Failed | JobStatus::Cancelled)
        {
            job.status = JobStatus::Pending;
            job.retry_count = 0;
            job.error_message = None;
            job.worker_id = None;
            job.started_at = None;
            job.completed_at = None;
            job.extraction_id = None;
            job.next_retry_at = None;
            job.updated_at = Utc::now();
            return Ok(Some(job.clone()));
        }
        Ok(None)
    }

    async fn release_job(&self, job_id: Uuid) -> Result<(), AppError> {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.iter_mut().find(|j| j.id == job_id) {
            job.status = JobStatus::Pending;
            job.worker_id = None;
        }
        Ok(())
    }

    async fn release_worker_jobs(&self, worker_id: &str) -> Result<u64, AppError> {
        self.released_workers
            .lock()
            .unwrap()
            .push(worker_id.to_string());

        let mut jobs = self.jobs.lock().unwrap();
        let mut count = 0u64;
        for job in jobs.iter_mut() {
            if job.worker_id.as_deref() == Some(worker_id) && job.status == JobStatus::Running {
                job.status = JobStatus::Pending;
                job.worker_id = None;
                count += 1;
            }
        }
        Ok(count)
    }

    async fn count_by_status(&self, status: JobStatus) -> Result<i64, AppError> {
        let jobs = self.jobs.lock().unwrap();
        Ok(jobs.iter().filter(|j| j.status == status).count() as i64)
    }

    async fn mark_url_visited(&self, session_id: Uuid, url: &str) -> Result<bool, AppError> {
        let mut visited = self.visited_urls.lock().unwrap();
        if visited.iter().any(|(s, u)| *s == session_id && u == url) {
            Ok(false)
        } else {
            visited.push((session_id, url.to_string()));
            Ok(true)
        }
    }

    async fn count_visited_urls(&self, session_id: Uuid) -> Result<i64, AppError> {
        let visited = self.visited_urls.lock().unwrap();
        Ok(visited.iter().filter(|(s, _)| *s == session_id).count() as i64)
    }
}

// ---------------------------------------------------------------------------
// MockReporter
// ---------------------------------------------------------------------------

/// Mock worker reporter that records events.
#[derive(Default)]
pub struct MockReporter {
    pub events: Arc<Mutex<Vec<String>>>,
}

impl MockReporter {
    pub fn new() -> Self {
        Self::default()
    }
}

impl crate::worker::WorkerReporter for MockReporter {
    fn report(&self, event: crate::worker::WorkerEvent<'_>) {
        let label = match &event {
            crate::worker::WorkerEvent::Started { .. } => "Started",
            crate::worker::WorkerEvent::Polling => "Polling",
            crate::worker::WorkerEvent::JobClaimed { .. } => "JobClaimed",
            crate::worker::WorkerEvent::JobStarted { .. } => "JobStarted",
            crate::worker::WorkerEvent::JobCompleted { .. } => "JobCompleted",
            crate::worker::WorkerEvent::JobFailed { .. } => "JobFailed",
            crate::worker::WorkerEvent::ShuttingDown { .. } => "ShuttingDown",
            crate::worker::WorkerEvent::Stopped { .. } => "Stopped",
        };
        self.events.lock().unwrap().push(label.to_string());
    }
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Create a dummy ScrapeJob for testing.
pub fn make_test_job() -> ScrapeJob {
    ScrapeJob {
        id: Uuid::new_v4(),
        url: "https://example.com".to_string(),
        schema_name: "test_schema".to_string(),
        schema: serde_json::json!({"type": "object", "properties": {"title": {"type": "string"}}}),
        model: "test-model".to_string(),
        base_url: "https://api.test.com/v1".to_string(),
        status: JobStatus::Pending,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        started_at: None,
        completed_at: None,
        retry_count: 0,
        max_retries: 3,
        next_retry_at: None,
        error_message: None,
        extraction_id: None,
        worker_id: None,
        crawl_session_id: None,
        parent_job_id: None,
        depth: 0,
        max_depth: 0,
        max_pages: 100,
        allowed_domains: Vec::new(),
    }
}

/// Create a dummy Extraction for testing (e.g., as a "previous" extraction).
pub fn make_test_extraction(data_hash: &str) -> Extraction {
    Extraction {
        id: Uuid::new_v4(),
        url: "https://example.com".to_string(),
        schema_name: "test_schema".to_string(),
        extracted_data: serde_json::json!({"title": "Test"}),
        content_hash: "abc123".to_string(),
        data_hash: data_hash.to_string(),
        model: "test-model".to_string(),
        created_at: Utc::now(),
    }
}
