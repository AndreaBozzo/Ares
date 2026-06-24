-- Ares: extraction run metadata (v0.5.0 — Evaluation & Reliability)
--
-- Enriches successful, persisted extractions with the provenance and cost
-- signals needed to make extraction measurable. The `extractions` table stays
-- valid-only (invalid output is never persisted here); the eval matrix incl.
-- invalids lives in a separate table.

ALTER TABLE extractions
    ADD COLUMN IF NOT EXISTS provider          VARCHAR(50) NOT NULL DEFAULT 'openai',
    ADD COLUMN IF NOT EXISTS schema_version    VARCHAR(50),
    ADD COLUMN IF NOT EXISTS latency_ms        BIGINT,
    ADD COLUMN IF NOT EXISTS prompt_tokens     INTEGER,
    ADD COLUMN IF NOT EXISTS completion_tokens INTEGER;
