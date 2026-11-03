CREATE TABLE IF NOT EXISTS accounts (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL UNIQUE COLLATE NOCASE,
    password_hash TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS profiles (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL
        REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    is_child INTEGER NOT NULL DEFAULT 0,
    age_rating_max INTEGER,
    UNIQUE(account_id, name)
);

CREATE TABLE IF NOT EXISTS auth_sessions (
    token_hash TEXT PRIMARY KEY,
    account_id TEXT NOT NULL
        REFERENCES accounts(id) ON DELETE CASCADE,
    profile_id TEXT NOT NULL
        REFERENCES profiles(id) ON DELETE CASCADE,
    expires_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_auth_sessions_expiry
ON auth_sessions(expires_at);
