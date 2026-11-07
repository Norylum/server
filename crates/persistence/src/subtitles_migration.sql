CREATE TABLE IF NOT EXISTS subtitle_tracks (
    id           TEXT NOT NULL PRIMARY KEY,
    media_id     TEXT NOT NULL,
    lang         TEXT NOT NULL,
    format       TEXT NOT NULL CHECK(format IN ('srt', 'vtt')),
    relative_path TEXT NOT NULL UNIQUE
);

CREATE INDEX IF NOT EXISTS idx_subtitle_tracks_media_id ON subtitle_tracks (media_id);
