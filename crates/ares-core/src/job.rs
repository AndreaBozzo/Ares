use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Status of a scrape job in the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Pending => "pending",
            JobStatus::Running => "running",
            JobStatus::Completed => "completed",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled
        )
    }
}

impl fmt::Display for JobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for JobStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pending" => Ok(JobStatus::Pending),
            "running" => Ok(JobStatus::Running),
            "completed" => Ok(JobStatus::Completed),
            "failed" => Ok(JobStatus::Failed),
            "cancelled" => Ok(JobStatus::Cancelled),
            _ => Err(format!("Unknown job status: {}", s)),
        }
    }
}

/// Retry configuration with exponential backoff.
///
/// Delay schedule: 1min, 5min, 30min, 60min (capped).
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub max_delay: TimeDelta,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            max_delay: TimeDelta::minutes(60),
        }
    }
}

impl RetryConfig {
    /// Calculate delay for a given attempt number (1-indexed).
    ///
    /// - Attempt 1: 1 minute
    /// - Attempt 2: 5 minutes
    /// - Attempt 3: 30 minutes
    /// - Attempt 4+: 60 minutes (capped by max_delay)
    pub fn delay_for_attempt(&self, attempt: u32) -> TimeDelta {
        let delay = match attempt {
            0 | 1 => TimeDelta::minutes(1),
            2 => TimeDelta::minutes(5),
            3 => TimeDelta::minutes(30),
            _ => TimeDelta::minutes(60),
        };
        std::cmp::min(delay, self.max_delay)
    }
}

/// A scrape job in the queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrapeJob {
    pub id: Uuid,
    pub url: String,
    pub schema_name: String,
    pub schema: serde_json::Value,
    pub model: String,
    pub base_url: String,
    pub status: JobStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub retry_count: u32,
    pub max_retries: u32,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
    pub extraction_id: Option<Uuid>,
    pub worker_id: Option<String>,
}

impl ScrapeJob {
    pub fn can_retry(&self) -> bool {
        self.retry_count < self.max_retries
    }

    pub fn calculate_next_retry(&self, config: &RetryConfig) -> DateTime<Utc> {
        let delay = config.delay_for_attempt(self.retry_count + 1);
        Utc::now() + delay
    }
}

/// Request to create a new scrape job.
#[derive(Debug, Clone)]
pub struct CreateScrapeJobRequest {
    pub url: String,
    pub schema_name: String,
    pub schema: serde_json::Value,
    pub model: String,
    pub base_url: String,
    pub max_retries: Option<u32>,
}

impl CreateScrapeJobRequest {
    pub fn new(
        url: impl Into<String>,
        schema_name: impl Into<String>,
        schema: serde_json::Value,
        model: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            url: url.into(),
            schema_name: schema_name.into(),
            schema,
            model: model.into(),
            base_url: base_url.into(),
            max_retries: None,
        }
    }

    pub fn with_max_retries(mut self, max: u32) -> Self {
        self.max_retries = Some(max);
        self
    }
}

/// Configuration for a worker process.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub worker_id: String,
    pub poll_interval: Duration,
    pub retry_config: RetryConfig,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            worker_id: format!("worker-{}", &Uuid::new_v4().to_string()[..8]),
            poll_interval: Duration::from_secs(5),
            retry_config: RetryConfig::default(),
        }
    }
}

impl WorkerConfig {
    pub fn with_worker_id(mut self, id: impl Into<String>) -> Self {
        self.worker_id = id.into();
        self
    }

    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_status_roundtrip() {
        for status in [
            JobStatus::Pending,
            JobStatus::Running,
            JobStatus::Completed,
            JobStatus::Failed,
            JobStatus::Cancelled,
        ] {
            let s = status.as_str();
            let parsed: JobStatus = s.parse().unwrap();
            assert_eq!(parsed, status);
        }
    }

    #[test]
    fn test_terminal_states() {
        assert!(!JobStatus::Pending.is_terminal());
        assert!(!JobStatus::Running.is_terminal());
        assert!(JobStatus::Completed.is_terminal());
        assert!(JobStatus::Failed.is_terminal());
        assert!(JobStatus::Cancelled.is_terminal());
    }

    #[test]
    fn test_retry_delay_schedule() {
        let config = RetryConfig::default();
        assert_eq!(config.delay_for_attempt(1), TimeDelta::minutes(1));
        assert_eq!(config.delay_for_attempt(2), TimeDelta::minutes(5));
        assert_eq!(config.delay_for_attempt(3), TimeDelta::minutes(30));
        assert_eq!(config.delay_for_attempt(4), TimeDelta::minutes(60));
    }

    #[test]
    fn test_create_job_request_builder() {
        let req = CreateScrapeJobRequest::new(
            "https://example.com",
            "test_schema",
            serde_json::json!({}),
            "gpt-4o-mini",
            "https://api.openai.com/v1",
        )
        .with_max_retries(5);

        assert_eq!(req.url, "https://example.com");
        assert_eq!(req.max_retries, Some(5));
    }
}
