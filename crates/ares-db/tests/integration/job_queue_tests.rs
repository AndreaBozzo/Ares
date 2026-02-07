use ares_core::job::{CreateScrapeJobRequest, JobStatus};
use ares_core::job_queue::JobQueue;
use ares_db::ScrapeJobRepository;

use crate::integration::common::setup_test_db;

fn test_request() -> CreateScrapeJobRequest {
    CreateScrapeJobRequest::new(
        "https://example.com",
        "blog",
        serde_json::json!({"type": "object"}),
        "gpt-4o-mini",
        "https://api.openai.com/v1",
    )
}

#[tokio::test]
async fn create_job_and_verify_fields() {
    let (pool, _container) = setup_test_db().await;
    let repo = ScrapeJobRepository::new(pool);

    let job = repo.create_job(test_request()).await.unwrap();

    assert_eq!(job.url, "https://example.com");
    assert_eq!(job.schema_name, "blog");
    assert_eq!(job.model, "gpt-4o-mini");
    assert_eq!(job.status, JobStatus::Pending);
    assert_eq!(job.retry_count, 0);
    assert_eq!(job.max_retries, 3);
    assert!(job.worker_id.is_none());
    assert!(job.started_at.is_none());
}

#[tokio::test]
async fn create_job_with_custom_max_retries() {
    let (pool, _container) = setup_test_db().await;
    let repo = ScrapeJobRepository::new(pool);

    let req = test_request().with_max_retries(10);
    let job = repo.create_job(req).await.unwrap();

    assert_eq!(job.max_retries, 10);
}

#[tokio::test]
async fn claim_job_sets_running_and_worker() {
    let (pool, _container) = setup_test_db().await;
    let repo = ScrapeJobRepository::new(pool);

    repo.create_job(test_request()).await.unwrap();

    let claimed = repo
        .claim_job("worker-1")
        .await
        .unwrap()
        .expect("Should claim the job");

    assert_eq!(claimed.status, JobStatus::Running);
    assert_eq!(claimed.worker_id.as_deref(), Some("worker-1"));
    assert!(claimed.started_at.is_some());
}

#[tokio::test]
async fn claim_job_returns_none_when_empty() {
    let (pool, _container) = setup_test_db().await;
    let repo = ScrapeJobRepository::new(pool);

    let claimed = repo.claim_job("worker-1").await.unwrap();
    assert!(claimed.is_none());
}

#[tokio::test]
async fn claim_job_skips_running_jobs() {
    let (pool, _container) = setup_test_db().await;
    let repo = ScrapeJobRepository::new(pool);

    repo.create_job(test_request()).await.unwrap();

    // First claim succeeds
    let claimed = repo.claim_job("worker-1").await.unwrap();
    assert!(claimed.is_some());

    // Second claim returns None (no pending jobs left)
    let claimed2 = repo.claim_job("worker-2").await.unwrap();
    assert!(claimed2.is_none());
}

#[tokio::test]
async fn complete_job_sets_completed_status() {
    let (pool, _container) = setup_test_db().await;
    let extraction_repo = ares_db::ExtractionRepository::new(pool.clone());
    let repo = ScrapeJobRepository::new(pool);

    // Create a real extraction first (FK constraint)
    let extraction = ares_core::models::NewExtraction {
        url: "https://example.com".into(),
        schema_name: "blog".into(),
        extracted_data: serde_json::json!({"title": "Test"}),
        raw_content_hash: "hash".into(),
        data_hash: "dhash".into(),
        model: "model".into(),
    };
    let extraction_id = extraction_repo.save(&extraction).await.unwrap();

    let job = repo.create_job(test_request()).await.unwrap();
    let claimed = repo.claim_job("worker-1").await.unwrap().unwrap();

    repo.complete_job(claimed.id, Some(extraction_id))
        .await
        .unwrap();

    let updated = repo.get_job(job.id).await.unwrap().unwrap();
    assert_eq!(updated.status, JobStatus::Completed);
    assert_eq!(updated.extraction_id, Some(extraction_id));
    assert!(updated.completed_at.is_some());
    assert!(updated.worker_id.is_none());
}

#[tokio::test]
async fn fail_job_with_retry_resets_to_pending() {
    let (pool, _container) = setup_test_db().await;
    let repo = ScrapeJobRepository::new(pool);

    let job = repo.create_job(test_request()).await.unwrap();
    repo.claim_job("worker-1").await.unwrap();

    let next_retry = chrono::Utc::now() + chrono::TimeDelta::minutes(5);
    repo.fail_job(job.id, "temporary error", Some(next_retry))
        .await
        .unwrap();

    let updated = repo.get_job(job.id).await.unwrap().unwrap();
    assert_eq!(updated.status, JobStatus::Pending);
    assert_eq!(updated.retry_count, 1);
    assert!(updated.next_retry_at.is_some());
    assert_eq!(updated.error_message.as_deref(), Some("temporary error"));
    assert!(updated.worker_id.is_none());
}

#[tokio::test]
async fn fail_job_without_retry_marks_failed() {
    let (pool, _container) = setup_test_db().await;
    let repo = ScrapeJobRepository::new(pool);

    let job = repo.create_job(test_request()).await.unwrap();
    repo.claim_job("worker-1").await.unwrap();

    repo.fail_job(job.id, "permanent error", None)
        .await
        .unwrap();

    let updated = repo.get_job(job.id).await.unwrap().unwrap();
    assert_eq!(updated.status, JobStatus::Failed);
    assert_eq!(updated.retry_count, 0); // Not incremented for permanent failure
    assert_eq!(updated.error_message.as_deref(), Some("permanent error"));
}

#[tokio::test]
async fn cancel_job_sets_cancelled() {
    let (pool, _container) = setup_test_db().await;
    let repo = ScrapeJobRepository::new(pool);

    let job = repo.create_job(test_request()).await.unwrap();

    repo.cancel_job(job.id).await.unwrap();

    let updated = repo.get_job(job.id).await.unwrap().unwrap();
    assert_eq!(updated.status, JobStatus::Cancelled);
}

#[tokio::test]
async fn cancel_job_ignores_completed() {
    let (pool, _container) = setup_test_db().await;
    let repo = ScrapeJobRepository::new(pool);

    let job = repo.create_job(test_request()).await.unwrap();
    repo.claim_job("worker-1").await.unwrap();
    repo.complete_job(job.id, None).await.unwrap();

    // Cancel should be a no-op
    repo.cancel_job(job.id).await.unwrap();

    let updated = repo.get_job(job.id).await.unwrap().unwrap();
    assert_eq!(updated.status, JobStatus::Completed);
}

#[tokio::test]
async fn release_worker_jobs_on_shutdown() {
    let (pool, _container) = setup_test_db().await;
    let repo = ScrapeJobRepository::new(pool);

    // Create and claim two jobs
    repo.create_job(test_request()).await.unwrap();
    repo.create_job(test_request()).await.unwrap();

    repo.claim_job("worker-1").await.unwrap();
    repo.claim_job("worker-1").await.unwrap();

    let released = repo.release_worker_jobs("worker-1").await.unwrap();
    assert_eq!(released, 2);

    // Both should be pending again
    let pending = repo.count_by_status(JobStatus::Pending).await.unwrap();
    assert_eq!(pending, 2);
}

#[tokio::test]
async fn list_jobs_with_status_filter() {
    let (pool, _container) = setup_test_db().await;
    let repo = ScrapeJobRepository::new(pool);

    repo.create_job(test_request()).await.unwrap();
    repo.create_job(test_request()).await.unwrap();
    repo.claim_job("worker-1").await.unwrap();

    let pending = repo.list_jobs(Some(JobStatus::Pending), 10).await.unwrap();
    assert_eq!(pending.len(), 1);

    let running = repo.list_jobs(Some(JobStatus::Running), 10).await.unwrap();
    assert_eq!(running.len(), 1);

    let all = repo.list_jobs(None, 10).await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn count_by_status() {
    let (pool, _container) = setup_test_db().await;
    let repo = ScrapeJobRepository::new(pool);

    repo.create_job(test_request()).await.unwrap();
    repo.create_job(test_request()).await.unwrap();
    repo.create_job(test_request()).await.unwrap();

    assert_eq!(repo.count_by_status(JobStatus::Pending).await.unwrap(), 3);
    assert_eq!(repo.count_by_status(JobStatus::Running).await.unwrap(), 0);
}
