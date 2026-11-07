-- Acquisition V4 : support des sources BitTorrent.
-- Exécuté une seule fois via "CREATE TABLE IF NOT EXISTS" + "ADD COLUMN IF NOT EXISTS".

ALTER TABLE acquisition_sources ADD COLUMN source_type TEXT NOT NULL DEFAULT 'https';
