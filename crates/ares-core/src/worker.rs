use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerError};
use crate::error::AppError;
use crate::job::{CreateScrapeJobRequest, ScrapeJob, WorkerConfig};
use crate::job_queue::JobQueue;
use crate::scrape::ScrapeService;
use crate::traits::{
    Cleaner, ExtractionStore, ExtractorFactory, Fetcher, LinkDiscoverer, RobotsChecker,
};

/// Events emitted by the worker for monitoring/logging.
#[derive(Debug, Clone)]
pub enum WorkerEvent<'a> {
    Started {
        worker_id: &'a str,
    },
    Polling,
    JobClaimed {
        job: &'a ScrapeJob,
    },
    JobStarted {
        job_id: Uuid,
        url: &'a str,
    },
    JobCompleted {
        job_id: Uuid,
        extraction_id: Option<Uuid>,
    },
    JobFailed {
        job_id: Uuid,
        error: &'a str,
        will_retry: bool,
    },
    ShuttingDown {
        worker_id: &'a str,
        jobs_released: u64,
    },
    Stopped {
        worker_id: &'a str,
    },
}

/// Trait for receiving worker events (decoupled logging).
pub trait WorkerReporter: Send + Sync {
    fn report(&self, event: WorkerEvent<'_>) {
        let _ = event;
    }
}

/// Reporter that uses the `tracing` crate.
#[derive(Debug, Default, Clone, Copy)]
pub struct TracingWorkerReporter;

impl WorkerReporter for TracingWorkerReporter {
    fn report(&self, event: WorkerEvent<'_>) {
        match event {
            WorkerEvent::Started { worker_id } => {
                tracing::info!(%worker_id, "Worker started");
            }
            WorkerEvent::Polling => {
                tracing::debug!("Polling for jobs");
            }
            WorkerEvent::JobClaimed { job } => {
                tracing::info!(job_id = %job.id, url = %job.url, "Job claimed");
            }
            WorkerEvent::JobStarted { job_id, url } => {
                tracing::info!(%job_id, %url, "Processing job");
            }
            WorkerEvent::JobCompleted {
                job_id,
                extraction_id,
            } => {
                tracing::info!(%job_id, ?extraction_id, "Job completed");
            }
            WorkerEvent::JobFailed {
                job_id,
                error,
                will_retry,
            } => {
                tracing::warn!(%job_id, %error, %will_retry, "Job failed");
            }
            WorkerEvent::ShuttingDown {
                worker_id,
                jobs_released,
            } => {
                tracing::info!(%worker_id, %jobs_released, "Worker shutting down");
            }
            WorkerEvent::Stopped { worker_id } => {
                tracing::info!(%worker_id, "Worker stopped");
            }
        }
    }
}

/// Worker that polls the job queue and processes scrape jobs.
pub struct WorkerService<Q, F, C, EF, S, LD, RC>
where
    Q: JobQueue,
    F: Fetcher,
    C: Cleaner,
    EF: ExtractorFactory,
    S: ExtractionStore,
    LD: LinkDiscoverer,
    RC: RobotsChecker,
{
    queue: Q,
    fetcher: F,
    cleaner: C,
    extractor_factory: EF,
    store: S,
    link_discoverer: LD,
    robots_checker: RC,
    circuit_breaker: CircuitBreaker,
    config: WorkerConfig,
}

impl<Q, F, C, EF, S, LD, RC> WorkerService<Q, F, C, EF, S, LD, RC>
where
    Q: JobQueue,
    F: Fetcher,
    C: Cleaner,
    EF: ExtractorFactory,
    S: ExtractionStore,
    LD: LinkDiscoverer,
    RC: RobotsChecker,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        queue: Q,
        fetcher: F,
        cleaner: C,
        extractor_factory: EF,
        store: S,
        link_discoverer: LD,
        robots_checker: RC,
        circuit_breaker: CircuitBreaker,
        config: WorkerConfig,
    ) -> Self {
        Self {
            queue,
            fetcher,
            cleaner,
            extractor_factory,
            store,
            link_discoverer,
            robots_checker,
            circuit_breaker,
            config,
        }
    }

    /// Run the worker loop until cancellation.
    pub async fn run<WR: WorkerReporter>(
        &self,
        cancel_token: CancellationToken,
        reporter: &WR,
    ) -> Result<(), AppError> {
        reporter.report(WorkerEvent::Started {
            worker_id: &self.config.worker_id,
        });

        loop {
            if cancel_token.is_cancelled() {
                break;
            }

            reporter.report(WorkerEvent::Polling);

            match self.queue.claim_job(&self.config.worker_id).await {
                Ok(Some(job)) => {
                    reporter.report(WorkerEvent::JobClaimed { job: &job });
                    self.process_job(&job, reporter).await;
                }
                Ok(None) => {
                    tokio::select! {
                        () = tokio::time::sleep(self.config.poll_interval) => {}
                        () = cancel_token.cancelled() => break,
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to claim job");
                    tokio::select! {
                        () = tokio::time::sleep(self.config.poll_interval * 2) => {}
                        () = cancel_token.cancelled() => break,
                    }
                }
            }
        }

        // Graceful shutdown: release all claimed jobs
        let released = self
            .queue
            .release_worker_jobs(&self.config.worker_id)
            .await
            .unwrap_or(0);

        reporter.report(WorkerEvent::ShuttingDown {
            worker_id: &self.config.worker_id,
            jobs_released: released,
        });
        reporter.report(WorkerEvent::Stopped {
            worker_id: &self.config.worker_id,
        });

        Ok(())
    }

    /// Process a single job. Public for testing purposes.
    pub async fn process_job<WR: WorkerReporter>(&self, job: &ScrapeJob, reporter: &WR) {
        reporter.report(WorkerEvent::JobStarted {
            job_id: job.id,
            url: &job.url,
        });

        // Create extractor for this job's model/base_url
        let extractor = match self.extractor_factory.create(&job.model, &job.base_url) {
            Ok(e) => e,
            Err(e) => {
                let error_msg = e.to_string();
                reporter.report(WorkerEvent::JobFailed {
                    job_id: job.id,
                    error: &error_msg,
                    will_retry: false,
                });
                let _ = self.queue.fail_job(job.id, &error_msg, None).await;
                return;
            }
        };

        // Build ScrapeService for this job
        let service = ScrapeService::with_store(
            self.fetcher.clone(),
            self.cleaner.clone(),
            extractor,
            self.store.clone(),
            job.model.clone(),
        )
        .with_skip_unchanged(self.config.skip_unchanged);

        // Wrap in circuit breaker
        let result = self
            .circuit_breaker
            .call(|| async {
                service
                    .scrape(&job.url, &job.schema, &job.schema_name)
                    .await
            })
            .await;

        match result {
            Ok(scrape_result) => {
                reporter.report(WorkerEvent::JobCompleted {
                    job_id: job.id,
                    extraction_id: scrape_result.extraction_id,
                });
                if let Err(e) = self
                    .queue
                    .complete_job(job.id, scrape_result.extraction_id)
                    .await
                {
                    tracing::error!(job_id = %job.id, error = %e, "Failed to mark job completed");
                }

                // --- SMART CRAWLING (Spidering) ---
                if let (Some(session_id), Some(html)) =
                    (job.crawl_session_id, scrape_result.raw_html)
                    && job.depth < job.max_depth
                {
                    match self.link_discoverer.discover_links(&html, &job.url) {
                        Ok(links) => {
                            // Determine allowed domains (default to seed URL's domain)
                            let allowed_domains = if job.allowed_domains.is_empty() {
                                Url::parse(&job.url)
                                    .ok()
                                    .and_then(|u| u.host_str().map(String::from))
                                    .into_iter()
                                    .collect::<Vec<_>>()
                            } else {
                                job.allowed_domains.clone()
                            };

                            // Count visited URLs once, track locally for max_pages
                            let mut visited_count = match self
                                .queue
                                .count_visited_urls(session_id)
                                .await
                            {
                                Ok(c) => c,
                                Err(e) => {
                                    tracing::error!(%session_id, error = %e, "Failed to count visited URLs");
                                    0
                                }
                            };

                            for link in links {
                                // 1. Max pages check
                                if visited_count >= job.max_pages as i64 {
                                    tracing::info!(
                                        %session_id,
                                        max_pages = job.max_pages,
                                        "Crawl reached max_pages limit"
                                    );
                                    break;
                                }

                                // 2. Domain filter
                                let link_domain = Url::parse(&link)
                                    .ok()
                                    .and_then(|u| u.host_str().map(String::from));
                                let Some(domain) = link_domain else {
                                    continue;
                                };
                                if !allowed_domains
                                    .iter()
                                    .any(|d| domain == *d || domain.ends_with(&format!(".{d}")))
                                {
                                    continue;
                                }

                                // 3. robots.txt check
                                if !self.robots_checker.is_allowed(&link).await {
                                    tracing::debug!(
                                        url = %link,
                                        "Skipping URL disallowed by robots.txt"
                                    );
                                    continue;
                                }

                                // 4. Atomic dedup: mark visited, skip if already seen
                                match self.queue.mark_url_visited(session_id, &link).await {
                                    Ok(true) => {
                                        visited_count += 1;

                                        // 4. Enqueue child job
                                        let request = CreateScrapeJobRequest::new(
                                            link,
                                            &job.schema_name,
                                            job.schema.clone(),
                                            &job.model,
                                            &job.base_url,
                                        )
                                        .with_crawl_context(
                                            session_id,
                                            Some(job.id),
                                            job.depth + 1,
                                            job.max_depth,
                                        )
                                        .with_crawl_config(
                                            job.max_pages,
                                            job.allowed_domains.clone(),
                                        );

                                        if let Err(e) = self.queue.create_job(request).await {
                                            tracing::error!(
                                                %session_id,
                                                error = %e,
                                                "Failed to create child crawl job"
                                            );
                                        }
                                    }
                                    Ok(false) => continue, // Already visited
                                    Err(e) => {
                                        tracing::error!(
                                            %session_id,
                                            error = %e,
                                            "Failed to mark URL visited"
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                job_id = %job.id,
                                error = %e,
                                "Link discovery failed for crawl job"
                            );
                        }
                    }
                }
            }
            Err(circuit_err) => {
                let (error_msg, is_retryable) = match &circuit_err {
                    CircuitBreakerError::Open {
                        name, retry_after, ..
                    } => (
                        format!(
                            "Circuit breaker '{}' open, retry after {}s",
                            name,
                            retry_after.as_secs()
                        ),
                        true,
                    ),
                    CircuitBreakerError::Inner(e) => (e.to_string(), e.is_retryable()),
                };

                let can_retry = job.can_retry() && is_retryable;
                reporter.report(WorkerEvent::JobFailed {
                    job_id: job.id,
                    error: &error_msg,
                    will_retry: can_retry,
                });

                let next_retry = if can_retry {
                    Some(job.calculate_next_retry(&self.config.retry_config))
                } else {
                    None
                };

                if let Err(e) = self.queue.fail_job(job.id, &error_msg, next_retry).await {
                    tracing::error!(job_id = %job.id, error = %e, "Failed to mark job as failed");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit_breaker::CircuitBreakerConfig;
    use crate::job::{RetryConfig, WorkerConfig};
    use crate::testutil::*;
    use std::time::Duration;

    fn test_config() -> WorkerConfig {
        WorkerConfig {
            worker_id: "test-worker".into(),
            poll_interval: Duration::from_millis(10),
            retry_config: RetryConfig::default(),
            skip_unchanged: false,
        }
    }

    fn test_cb() -> CircuitBreaker {
        CircuitBreaker::new("test", CircuitBreakerConfig::default())
    }

    #[tokio::test]
    async fn process_job_successfully_completes() {
        let job = make_test_job();
        let queue = MockJobQueue::with_job(job.clone());
        let reporter = MockReporter::new();

        let worker = WorkerService::new(
            queue.clone(),
            MockFetcher::new("<html>hi</html>"),
            MockCleaner::passthrough(),
            MockExtractorFactory::new(serde_json::json!({"title": "Test"})),
            MockStore::empty(),
            MockLinkDiscoverer::new(),
            MockRobotsChecker::new(),
            test_cb(),
            test_config(),
        );

        worker.process_job(&job, &reporter).await;

        let completed = queue.completed_jobs.lock().unwrap();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].0, job.id);
        assert!(completed[0].1.is_some()); // extraction_id

        let events = reporter.events.lock().unwrap();
        assert!(events.contains(&"JobStarted".to_string()));
        assert!(events.contains(&"JobCompleted".to_string()));
    }

    #[tokio::test]
    async fn process_job_retryable_error_schedules_retry() {
        let job = make_test_job();
        let queue = MockJobQueue::with_job(job.clone());
        let reporter = MockReporter::new();

        let worker = WorkerService::new(
            queue.clone(),
            MockFetcher::with_error(AppError::NetworkError("timeout".into())),
            MockCleaner::passthrough(),
            MockExtractorFactory::new(serde_json::json!({})),
            MockStore::empty(),
            MockLinkDiscoverer::new(),
            MockRobotsChecker::new(),
            test_cb(),
            test_config(),
        );

        worker.process_job(&job, &reporter).await;

        let failed = queue.failed_jobs.lock().unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].0, job.id);
        assert!(
            failed[0].2.is_some(),
            "Should have next_retry_at for retryable error"
        );

        let events = reporter.events.lock().unwrap();
        assert!(events.contains(&"JobFailed".to_string()));
    }

    #[tokio::test]
    async fn process_job_non_retryable_error_fails_permanently() {
        let job = make_test_job();
        let queue = MockJobQueue::with_job(job.clone());
        let reporter = MockReporter::new();

        let worker = WorkerService::new(
            queue.clone(),
            MockFetcher::new("<html>hi</html>"),
            MockCleaner::with_error(AppError::CleanerError("bad html".into())),
            MockExtractorFactory::new(serde_json::json!({})),
            MockStore::empty(),
            MockLinkDiscoverer::new(),
            MockRobotsChecker::new(),
            test_cb(),
            test_config(),
        );

        worker.process_job(&job, &reporter).await;

        let failed = queue.failed_jobs.lock().unwrap();
        assert_eq!(failed.len(), 1);
        assert!(
            failed[0].2.is_none(),
            "Should NOT have next_retry_at for non-retryable error"
        );
    }

    #[tokio::test]
    async fn process_job_circuit_open_retries() {
        let job = make_test_job();
        let queue = MockJobQueue::with_job(job.clone());
        let reporter = MockReporter::new();

        let cb_config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_secs(60),
            ..Default::default()
        };
        let cb = CircuitBreaker::new("test", cb_config);
        cb.record_failure(&AppError::NetworkError("test".into()));

        let worker = WorkerService::new(
            queue.clone(),
            MockFetcher::new("<html>hi</html>"),
            MockCleaner::passthrough(),
            MockExtractorFactory::new(serde_json::json!({})),
            MockStore::empty(),
            MockLinkDiscoverer::new(),
            MockRobotsChecker::new(),
            cb,
            test_config(),
        );

        worker.process_job(&job, &reporter).await;

        let failed = queue.failed_jobs.lock().unwrap();
        assert_eq!(failed.len(), 1);
        assert!(
            failed[0].2.is_some(),
            "Circuit open error should schedule retry"
        );
    }

    #[tokio::test]
    async fn process_job_factory_error_fails_without_retry() {
        let job = make_test_job();
        let queue = MockJobQueue::with_job(job.clone());
        let reporter = MockReporter::new();

        let worker = WorkerService::new(
            queue.clone(),
            MockFetcher::new("<html>hi</html>"),
            MockCleaner::passthrough(),
            MockExtractorFactory::with_create_error(AppError::Generic("bad model config".into())),
            MockStore::empty(),
            MockLinkDiscoverer::new(),
            MockRobotsChecker::new(),
            test_cb(),
            test_config(),
        );

        worker.process_job(&job, &reporter).await;

        let failed = queue.failed_jobs.lock().unwrap();
        assert_eq!(failed.len(), 1);
        assert!(
            failed[0].2.is_none(),
            "Factory error should not schedule retry"
        );

        let events = reporter.events.lock().unwrap();
        assert!(events.contains(&"JobFailed".to_string()));
    }

    #[tokio::test]
    async fn run_loop_graceful_shutdown_releases_jobs() {
        let queue = MockJobQueue::empty();
        let reporter = MockReporter::new();
        let cancel = CancellationToken::new();

        let worker = WorkerService::new(
            queue.clone(),
            MockFetcher::new("<html>hi</html>"),
            MockCleaner::passthrough(),
            MockExtractorFactory::new(serde_json::json!({})),
            MockStore::empty(),
            MockLinkDiscoverer::new(),
            MockRobotsChecker::new(),
            test_cb(),
            test_config(),
        );

        cancel.cancel();

        worker.run(cancel, &reporter).await.unwrap();

        let released = queue.released_workers.lock().unwrap();
        assert_eq!(released.len(), 1);
        assert_eq!(released[0], "test-worker");

        let events = reporter.events.lock().unwrap();
        assert!(events.contains(&"Started".to_string()));
        assert!(events.contains(&"ShuttingDown".to_string()));
        assert!(events.contains(&"Stopped".to_string()));
    }

    #[tokio::test]
    async fn run_loop_processes_job_then_shuts_down() {
        let job = make_test_job();
        let queue = MockJobQueue::with_job(job.clone());
        let reporter = MockReporter::new();
        let cancel = CancellationToken::new();

        let worker = WorkerService::new(
            queue.clone(),
            MockFetcher::new("<html>hi</html>"),
            MockCleaner::passthrough(),
            MockExtractorFactory::new(serde_json::json!({"title": "Test"})),
            MockStore::empty(),
            MockLinkDiscoverer::new(),
            MockRobotsChecker::new(),
            test_cb(),
            test_config(),
        );

        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });

        worker.run(cancel, &reporter).await.unwrap();

        let completed = queue.completed_jobs.lock().unwrap();
        assert_eq!(completed.len(), 1);

        let events = reporter.events.lock().unwrap();
        assert!(events.contains(&"JobCompleted".to_string()));
        assert!(events.contains(&"Stopped".to_string()));
    }

    #[tokio::test]
    async fn retryable_error_but_max_retries_exceeded() {
        let mut job = make_test_job();
        job.retry_count = 3; // == max_retries, so can_retry() returns false
        let queue = MockJobQueue::with_job(job.clone());
        let reporter = MockReporter::new();

        let worker = WorkerService::new(
            queue.clone(),
            MockFetcher::with_error(AppError::NetworkError("timeout".into())),
            MockCleaner::passthrough(),
            MockExtractorFactory::new(serde_json::json!({})),
            MockStore::empty(),
            MockLinkDiscoverer::new(),
            MockRobotsChecker::new(),
            test_cb(),
            test_config(),
        );

        worker.process_job(&job, &reporter).await;

        let failed = queue.failed_jobs.lock().unwrap();
        assert_eq!(failed.len(), 1);
        assert!(
            failed[0].2.is_none(),
            "Should NOT schedule retry when max retries exceeded"
        );
    }

    #[tokio::test]
    async fn run_loop_claim_error_continues() {
        let queue = MockJobQueue::with_claim_error(AppError::DatabaseError("conn lost".into()));
        let reporter = MockReporter::new();
        let cancel = CancellationToken::new();

        let worker = WorkerService::new(
            queue.clone(),
            MockFetcher::new("<html>hi</html>"),
            MockCleaner::passthrough(),
            MockExtractorFactory::new(serde_json::json!({})),
            MockStore::empty(),
            MockLinkDiscoverer::new(),
            MockRobotsChecker::new(),
            test_cb(),
            test_config(),
        );

        // Cancel after a short delay — the worker should not crash on claim error
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });

        let result = worker.run(cancel, &reporter).await;
        assert!(
            result.is_ok(),
            "Worker should shut down gracefully after claim error"
        );

        let events = reporter.events.lock().unwrap();
        assert!(events.contains(&"Started".to_string()));
        assert!(events.contains(&"Stopped".to_string()));
    }

    #[tokio::test]
    async fn process_job_store_error_fails_job() {
        let job = make_test_job();
        let queue = MockJobQueue::with_job(job.clone());
        let reporter = MockReporter::new();

        let worker = WorkerService::new(
            queue.clone(),
            MockFetcher::new("<html>hi</html>"),
            MockCleaner::passthrough(),
            MockExtractorFactory::new(serde_json::json!({"title": "Test"})),
            MockStore::with_save_error(AppError::DatabaseError("disk full".into())),
            MockLinkDiscoverer::new(),
            MockRobotsChecker::new(),
            test_cb(),
            test_config(),
        );

        worker.process_job(&job, &reporter).await;

        let failed = queue.failed_jobs.lock().unwrap();
        assert_eq!(failed.len(), 1);
        assert!(failed[0].1.contains("disk full"));
    }

    // --- Crawl-specific tests ---

    fn make_crawl_job(
        session_id: Uuid,
        depth: u32,
        max_depth: u32,
        max_pages: u32,
        allowed_domains: Vec<String>,
    ) -> ScrapeJob {
        let mut job = make_test_job();
        job.crawl_session_id = Some(session_id);
        job.depth = depth;
        job.max_depth = max_depth;
        job.max_pages = max_pages;
        job.allowed_domains = allowed_domains;
        job
    }

    #[tokio::test]
    async fn crawl_job_enqueues_child_jobs() {
        let session_id = Uuid::new_v4();
        let job = make_crawl_job(session_id, 0, 2, 100, vec!["example.com".to_string()]);
        let queue = MockJobQueue::with_job(job.clone());
        let reporter = MockReporter::new();

        let worker = WorkerService::new(
            queue.clone(),
            MockFetcher::new("<html><a href='/page1'>1</a></html>"),
            MockCleaner::passthrough(),
            MockExtractorFactory::new(serde_json::json!({"title": "Test"})),
            MockStore::empty(),
            MockLinkDiscoverer::with_links(vec![
                "https://example.com/page1".to_string(),
                "https://example.com/page2".to_string(),
            ]),
            MockRobotsChecker::new(),
            test_cb(),
            test_config(),
        );

        worker.process_job(&job, &reporter).await;

        // Original job completed + 2 child jobs created
        let jobs = queue.jobs.lock().unwrap();
        let child_jobs: Vec<_> = jobs
            .iter()
            .filter(|j| j.parent_job_id == Some(job.id))
            .collect();
        assert_eq!(child_jobs.len(), 2);
        assert_eq!(child_jobs[0].depth, 1);
        assert_eq!(child_jobs[0].max_depth, 2);
        assert_eq!(child_jobs[0].crawl_session_id, Some(session_id));
    }

    #[tokio::test]
    async fn crawl_job_filters_external_domains() {
        let session_id = Uuid::new_v4();
        let job = make_crawl_job(session_id, 0, 2, 100, vec!["example.com".to_string()]);
        let queue = MockJobQueue::with_job(job.clone());
        let reporter = MockReporter::new();

        let worker = WorkerService::new(
            queue.clone(),
            MockFetcher::new("<html>hi</html>"),
            MockCleaner::passthrough(),
            MockExtractorFactory::new(serde_json::json!({"title": "Test"})),
            MockStore::empty(),
            MockLinkDiscoverer::with_links(vec![
                "https://example.com/page1".to_string(),
                "https://other.com/page2".to_string(),
                "https://sub.example.com/page3".to_string(),
            ]),
            MockRobotsChecker::new(),
            test_cb(),
            test_config(),
        );

        worker.process_job(&job, &reporter).await;

        let jobs = queue.jobs.lock().unwrap();
        let child_jobs: Vec<_> = jobs
            .iter()
            .filter(|j| j.parent_job_id == Some(job.id))
            .collect();
        // example.com/page1 and sub.example.com/page3 allowed; other.com filtered
        assert_eq!(child_jobs.len(), 2);
        let urls: Vec<_> = child_jobs.iter().map(|j| j.url.as_str()).collect();
        assert!(urls.contains(&"https://example.com/page1"));
        assert!(urls.contains(&"https://sub.example.com/page3"));
    }

    #[tokio::test]
    async fn crawl_job_respects_max_depth() {
        let session_id = Uuid::new_v4();
        // depth == max_depth, so no children should be created
        let job = make_crawl_job(session_id, 2, 2, 100, vec!["example.com".to_string()]);
        let queue = MockJobQueue::with_job(job.clone());
        let reporter = MockReporter::new();

        let worker = WorkerService::new(
            queue.clone(),
            MockFetcher::new("<html>hi</html>"),
            MockCleaner::passthrough(),
            MockExtractorFactory::new(serde_json::json!({"title": "Test"})),
            MockStore::empty(),
            MockLinkDiscoverer::with_links(vec!["https://example.com/page1".to_string()]),
            MockRobotsChecker::new(),
            test_cb(),
            test_config(),
        );

        worker.process_job(&job, &reporter).await;

        let jobs = queue.jobs.lock().unwrap();
        let child_jobs: Vec<_> = jobs
            .iter()
            .filter(|j| j.parent_job_id == Some(job.id))
            .collect();
        assert_eq!(child_jobs.len(), 0);
    }

    #[tokio::test]
    async fn crawl_job_deduplicates_urls() {
        let session_id = Uuid::new_v4();
        let job = make_crawl_job(session_id, 0, 2, 100, vec!["example.com".to_string()]);
        let queue = MockJobQueue::with_job(job.clone());
        // Pre-populate visited URL
        queue
            .visited_urls
            .lock()
            .unwrap()
            .push((session_id, "https://example.com/page1".to_string()));
        let reporter = MockReporter::new();

        let worker = WorkerService::new(
            queue.clone(),
            MockFetcher::new("<html>hi</html>"),
            MockCleaner::passthrough(),
            MockExtractorFactory::new(serde_json::json!({"title": "Test"})),
            MockStore::empty(),
            MockLinkDiscoverer::with_links(vec![
                "https://example.com/page1".to_string(), // already visited
                "https://example.com/page2".to_string(), // new
            ]),
            MockRobotsChecker::new(),
            test_cb(),
            test_config(),
        );

        worker.process_job(&job, &reporter).await;

        let jobs = queue.jobs.lock().unwrap();
        let child_jobs: Vec<_> = jobs
            .iter()
            .filter(|j| j.parent_job_id == Some(job.id))
            .collect();
        // Only page2 should be enqueued
        assert_eq!(child_jobs.len(), 1);
        assert_eq!(child_jobs[0].url, "https://example.com/page2");
    }

    #[tokio::test]
    async fn crawl_job_respects_max_pages() {
        let session_id = Uuid::new_v4();
        let job = make_crawl_job(
            session_id,
            0,
            2,
            2, // max 2 pages
            vec!["example.com".to_string()],
        );
        let queue = MockJobQueue::with_job(job.clone());
        // Pre-populate 2 visited URLs (at max already)
        {
            let mut visited = queue.visited_urls.lock().unwrap();
            visited.push((session_id, "https://example.com/seed".to_string()));
            visited.push((session_id, "https://example.com/page0".to_string()));
        }
        let reporter = MockReporter::new();

        let worker = WorkerService::new(
            queue.clone(),
            MockFetcher::new("<html>hi</html>"),
            MockCleaner::passthrough(),
            MockExtractorFactory::new(serde_json::json!({"title": "Test"})),
            MockStore::empty(),
            MockLinkDiscoverer::with_links(vec![
                "https://example.com/page1".to_string(),
                "https://example.com/page2".to_string(),
            ]),
            MockRobotsChecker::new(),
            test_cb(),
            test_config(),
        );

        worker.process_job(&job, &reporter).await;

        let jobs = queue.jobs.lock().unwrap();
        let child_jobs: Vec<_> = jobs
            .iter()
            .filter(|j| j.parent_job_id == Some(job.id))
            .collect();
        assert_eq!(child_jobs.len(), 0, "Should not enqueue when at max_pages");
    }

    #[tokio::test]
    async fn non_crawl_job_skips_link_discovery() {
        let job = make_test_job(); // no crawl_session_id
        let queue = MockJobQueue::with_job(job.clone());
        let reporter = MockReporter::new();

        let worker = WorkerService::new(
            queue.clone(),
            MockFetcher::new("<html><a href='/page1'>1</a></html>"),
            MockCleaner::passthrough(),
            MockExtractorFactory::new(serde_json::json!({"title": "Test"})),
            MockStore::empty(),
            MockLinkDiscoverer::with_links(vec!["https://example.com/page1".to_string()]),
            MockRobotsChecker::new(),
            test_cb(),
            test_config(),
        );

        worker.process_job(&job, &reporter).await;

        let jobs = queue.jobs.lock().unwrap();
        // Only the original job, no children
        assert_eq!(jobs.len(), 1);
        assert!(jobs[0].parent_job_id.is_none());
    }

    #[tokio::test]
    async fn crawl_job_respects_robots_txt() {
        let session_id = Uuid::new_v4();
        let job = make_crawl_job(session_id, 0, 2, 100, vec!["example.com".to_string()]);
        let queue = MockJobQueue::with_job(job.clone());
        let reporter = MockReporter::new();

        let worker = WorkerService::new(
            queue.clone(),
            MockFetcher::new("<html>hi</html>"),
            MockCleaner::passthrough(),
            MockExtractorFactory::new(serde_json::json!({"title": "Test"})),
            MockStore::empty(),
            MockLinkDiscoverer::with_links(vec![
                "https://example.com/public".to_string(),
                "https://example.com/admin/secret".to_string(),
            ]),
            MockRobotsChecker::with_blocked(vec!["/admin".to_string()]),
            test_cb(),
            test_config(),
        );

        worker.process_job(&job, &reporter).await;

        let jobs = queue.jobs.lock().unwrap();
        let child_jobs: Vec<_> = jobs
            .iter()
            .filter(|j| j.parent_job_id == Some(job.id))
            .collect();
        // Only /public should be enqueued, /admin/secret blocked by robots.txt
        assert_eq!(child_jobs.len(), 1);
        assert_eq!(child_jobs[0].url, "https://example.com/public");
    }
}
