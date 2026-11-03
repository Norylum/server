-- Additive, idempotent, and isolated from media storage.
CREATE TABLE IF NOT EXISTS acquisition_worker_leases (
    request_id TEXT PRIMARY KEY
        REFERENCES acquisition_requests(id) ON DELETE CASCADE,
    lease_token TEXT,
    expires_at TEXT,
    attempt_count INTEGER NOT NULL DEFAULT 0
        CHECK (attempt_count BETWEEN 0 AND 3),
    updated_at TEXT NOT NULL,
    CHECK (
        (lease_token IS NULL AND expires_at IS NULL)
        OR
        (lease_token IS NOT NULL AND expires_at IS NOT NULL)
    )
);

CREATE INDEX IF NOT EXISTS idx_acquisition_leases_expiry
ON acquisition_worker_leases(expires_at)
WHERE expires_at IS NOT NULL;
