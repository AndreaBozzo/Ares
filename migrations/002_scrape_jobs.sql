-- Scrape job queue for persistent, retryable job processing.

CREATE TABLE IF NOT EXISTS scrape_jobs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Job target
    url VARCHAR NOT NULL,
    schema_name VARCHAR NOT NULL,
    schema JSONB NOT NULL,

    -- LLM configuration
    model VARCHAR(100) NOT NULL,
    base_url VARCHAR NOT NULL DEFAULT 'https://api.openai.com/v1',

    -- Status: pending, running, completed, failed, cancelled
    status VARCHAR(20) NOT NULL DEFAULT 'pending',

    -- Timestamps
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,

    -- Retry handling
    retry_count INTEGER NOT NULL DEFAULT 0,
    max_retries INTEGER NOT NULL DEFAULT 3,
    next_retry_at TIMESTAMPTZ,

    -- Error tracking
    error_message TEXT,

    -- Result reference
    extraction_id UUID REFERENCES extractions(id),

    -- Worker identification
    worker_id VARCHAR(255),

    CONSTRAINT chk_scrape_jobs_status CHECK (
        status IN ('pending', 'running', 'completed', 'failed', 'cancelled')
    )
);

-- Efficient job claiming (pending jobs sorted by creation)
CREATE INDEX idx_scrape_jobs_pending
ON scrape_jobs(created_at)
WHERE status = 'pending';

-- Retry scheduling
CREATE INDEX idx_scrape_jobs_retry
ON scrape_jobs(next_retry_at)
WHERE status = 'pending' AND next_retry_at IS NOT NULL;

-- Worker's running jobs (graceful shutdown)
CREATE INDEX idx_scrape_jobs_worker
ON scrape_jobs(worker_id)
WHERE status = 'running';

-- List jobs by status
CREATE INDEX idx_scrape_jobs_status
ON scrape_jobs(status, created_at DESC);

-- URL-specific job lookup
CREATE INDEX idx_scrape_jobs_url
ON scrape_jobs(url, created_at DESC);
