CREATE TABLE IF NOT EXISTS acquisition_requests (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('movie', 'tv')),
    tmdb_id INTEGER NOT NULL CHECK (tmdb_id > 0),
    season INTEGER CHECK (season BETWEEN 0 AND 999),
    episode INTEGER CHECK (episode BETWEEN 1 AND 999),
    quality_profile TEXT NOT NULL
        CHECK (quality_profile IN ('720p', '1080p', '2160p')),
    language TEXT NOT NULL
        CHECK (language IN ('fr', 'en', 'original')),
    status TEXT NOT NULL DEFAULT 'awaiting_approval'
        CHECK (status IN (
            'awaiting_approval', 'approved',
            'searching', 'downloading', 'processing',
            'available', 'unavailable', 'failed', 'cancelled'
        )),
    account_id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (
        (kind = 'movie' AND season IS NULL AND episode IS NULL)
        OR
        (kind = 'tv' AND (episode IS NULL OR season IS NOT NULL))
    )
);

CREATE UNIQUE INDEX IF NOT EXISTS
    uq_acquisition_active_request
ON acquisition_requests (
    account_id,
    kind,
    tmdb_id,
    COALESCE(season, -1),
    COALESCE(episode, -1)
)
WHERE status IN (
    'awaiting_approval', 'approved',
    'searching', 'downloading', 'processing'
);

CREATE INDEX IF NOT EXISTS idx_acquisition_account_created
ON acquisition_requests (account_id, created_at DESC);
