CREATE TABLE IF NOT EXISTS torrent_shares (
    media_id TEXT PRIMARY KEY
        REFERENCES media_items(id) ON DELETE CASCADE,
    enabled INTEGER NOT NULL DEFAULT 0
        CHECK (enabled IN (0, 1)),
    status TEXT NOT NULL DEFAULT 'disabled',
    magnet_uri TEXT,
    uploaded_bytes INTEGER NOT NULL DEFAULT 0,
    upload_bps INTEGER NOT NULL DEFAULT 0,
    peer_count INTEGER,
    last_error TEXT,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS torrent_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    upload_kibps INTEGER NOT NULL DEFAULT 64
        CHECK (upload_kibps BETWEEN 8 AND 512),
    updated_at TEXT NOT NULL
);

INSERT OR IGNORE INTO torrent_settings
    (id, upload_kibps, updated_at)
VALUES (1, 64, datetime('now'));
