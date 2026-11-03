CREATE TABLE IF NOT EXISTS ingestion_jobs (
    source TEXT PRIMARY KEY,
    destination TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('processing','published','failed')),
    attempts INTEGER NOT NULL DEFAULT 0,
    media_id TEXT,
    last_error TEXT,
    updated_at TEXT NOT NULL
);
