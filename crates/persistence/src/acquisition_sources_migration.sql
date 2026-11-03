-- Only local, trusted administration can register a source.
-- No automatic source lookup or network access.

CREATE TABLE IF NOT EXISTS acquisition_sources (
    request_id TEXT PRIMARY KEY
        REFERENCES acquisition_requests(id) ON DELETE CASCADE,
    source_url TEXT NOT NULL
        CHECK (length(source_url) BETWEEN 12 AND 2048),
    expected_sha256 TEXT NOT NULL
        CHECK (length(expected_sha256) = 64),
    registered_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_acquisition_source_registered
ON acquisition_sources(registered_at);
