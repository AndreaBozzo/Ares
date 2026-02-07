use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerError};
use crate::error::AppError;
use crate::job::{ScrapeJob, WorkerConfig};
use crate::job_queue::JobQueue;
use crate::scrape::ScrapeService;
use crate::traits::{Cleaner, ExtractionStore, ExtractorFactory, Fetcher};

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
pub struct WorkerService<Q, F, C, EF, S>
where
    Q: JobQueue,
    F: Fetcher,
    C: Cleaner,
    EF: ExtractorFactory,
    S: ExtractionStore,
{
    queue: Q,
    fetcher: F,
    cleaner: C,
    extractor_factory: EF,
    store: S,
    circuit_breaker: CircuitBreaker,
    config: WorkerConfig,
}

impl<Q, F, C, EF, S> WorkerService<Q, F, C, EF, S>
where
    Q: JobQueue,
    F: Fetcher,
    C: Cleaner,
    EF: ExtractorFactory,
    S: ExtractionStore,
{
    pub fn new(
        queue: Q,
        fetcher: F,
        cleaner: C,
        extractor_factory: EF,
        store: S,
        circuit_breaker: CircuitBreaker,
        config: WorkerConfig,
    ) -> Self {
        Self {
            queue,
            fetcher,
            cleaner,
            extractor_factory,
            store,
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
        );

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
}
