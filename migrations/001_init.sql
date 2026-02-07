-- Ares: Initial schema for extraction persistence

-- Extraction results
CREATE TABLE IF NOT EXISTS extractions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    url VARCHAR NOT NULL,
    schema_name VARCHAR NOT NULL,
    extracted_data JSONB NOT NULL,
    raw_content_hash VARCHAR(64) NOT NULL,
    data_hash VARCHAR(64) NOT NULL,
    model VARCHAR(100) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_extractions_url
    ON extractions(url, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_extractions_url_schema
    ON extractions(url, schema_name, created_at DESC);
