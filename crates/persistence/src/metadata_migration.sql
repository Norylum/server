CREATE TABLE IF NOT EXISTS media_metadata (
    media_id TEXT PRIMARY KEY
        REFERENCES media_items(id) ON DELETE CASCADE,
    tmdb_id INTEGER NOT NULL CHECK (tmdb_id > 0),
    title TEXT NOT NULL,
    overview TEXT NOT NULL,
    poster_path TEXT,
    release_date TEXT,
    updated_at TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'TMDB'
        CHECK (source = 'TMDB'),
    provenance TEXT NOT NULL DEFAULT 'Norylum Server'
        CHECK (provenance = 'Norylum Server')
);

CREATE TABLE IF NOT EXISTS show_metadata (
    show_id TEXT PRIMARY KEY
        REFERENCES shows(id) ON DELETE CASCADE,
    tmdb_id INTEGER NOT NULL CHECK (tmdb_id > 0),
    title TEXT NOT NULL,
    overview TEXT NOT NULL,
    poster_path TEXT,
    release_date TEXT,
    updated_at TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'TMDB'
        CHECK (source = 'TMDB'),
    provenance TEXT NOT NULL DEFAULT 'Norylum Server'
        CHECK (provenance = 'Norylum Server')
);
