CREATE TABLE IF NOT EXISTS playback_sync_operations (
    account_id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    media_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    expected_revision INTEGER,
    position_seconds REAL NOT NULL,
    duration_seconds REAL NOT NULL,
    is_completed INTEGER NOT NULL,
    revision INTEGER NOT NULL,
    updated_at TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (
        account_id,
        profile_id,
        media_id,
        operation_id
    )
);

CREATE INDEX IF NOT EXISTS
    idx_playback_sync_operations_created_at
ON playback_sync_operations(created_at);
