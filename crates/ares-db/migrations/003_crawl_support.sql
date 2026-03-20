-- Crawl support: sessions, parent/child relationships, and deduplication.

-- 1. Extend scrape_jobs with crawl metadata
ALTER TABLE scrape_jobs
ADD COLUMN IF NOT EXISTS crawl_session_id UUID,
ADD COLUMN IF NOT EXISTS parent_job_id UUID REFERENCES scrape_jobs(id),
ADD COLUMN IF NOT EXISTS depth INTEGER NOT NULL DEFAULT 0,
ADD COLUMN IF NOT EXISTS max_depth INTEGER NOT NULL DEFAULT 0;

-- 2. Create visited URLs table for deduplication
CREATE TABLE IF NOT EXISTS crawl_visited_urls (
    session_id UUID NOT NULL,
    url_hash VARCHAR(64) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (session_id, url_hash)
);

-- Index for session-based job lookups
CREATE INDEX IF NOT EXISTS idx_scrape_jobs_crawl_session
ON scrape_jobs(crawl_session_id, created_at DESC);

-- Index for parent/child relationship
CREATE INDEX IF NOT EXISTS idx_scrape_jobs_parent
ON scrape_jobs(parent_job_id);
