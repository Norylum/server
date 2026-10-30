//! Crate `persistence` pour Norylum Server.
//! Gère la base de données SQLite local-first en mode WAL, l'exécution automatique des migrations
//! et la persistance de l'index du catalogue, des séries/saisons/épisodes et des progressions de lecture.

use chrono::{DateTime, Utc};
use norylum_foundation::{
    AccountId, EpisodeId, MediaId, NorylumError, ProfileId, SeasonId, ShowId,
};
use norylum_library::MediaItem;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Modèle de données pour la persistance de la progression de lecture.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlaybackProgressRecord {
    pub account_id: AccountId,
    pub profile_id: ProfileId,
    pub media_id: MediaId,
    pub position_seconds: f64,
    pub duration_seconds: f64,
    pub is_completed: bool,
    pub updated_at: DateTime<Utc>,
}

/// Modèle de données pour une série télévisée dans SQLite.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShowRecord {
    pub id: ShowId,
    pub title: String,
    pub year: Option<u16>,
    pub relative_path: String,
    pub is_child_safe: bool,
    pub age_rating: Option<u32>,
    pub season_count: u32,
    pub episode_count: u32,
    pub created_at: String,
    pub updated_at: String,
}

/// Modèle de données pour une saison dans SQLite.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SeasonRecord {
    pub id: SeasonId,
    pub show_id: ShowId,
    pub season_number: u32,
    pub title: String,
    pub relative_path: String,
    pub episode_count: u32,
    pub created_at: String,
    pub updated_at: String,
}

/// Modèle de données pour un épisode dans SQLite.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpisodeRecord {
    pub id: EpisodeId,
    pub season_id: SeasonId,
    pub show_id: ShowId,
    pub media_id: MediaId,
    pub episode_number: u32,
    pub episode_end: Option<u32>,
    pub title: String,
    pub duration_seconds: f64,
    pub is_child_safe: bool,
    pub age_rating: Option<u32>,
    pub created_at: String,
    pub updated_at: String,
}

/// Gestionnaire de persistance SQLite local-first pour Norylum.
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<rusqlite::Connection>>,
}

impl Database {
    /// Ouvre ou crée la base de données SQLite au chemin spécifié, active WAL et exécute les migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, NorylumError> {
        let p = path.as_ref();
        if let Some(parent) = p.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent).map_err(NorylumError::Io)?;
            }
        }

        let conn = rusqlite::Connection::open(p).map_err(|e| {
            NorylumError::Internal(format!(
                "Impossible d'ouvrir la base SQLite ({:?}): {}",
                p, e
            ))
        })?;

        Self::init_connection(conn)
    }

    /// Ouvre une base SQLite en mémoire vive (essentiellement pour les tests automatisés).
    pub fn in_memory() -> Result<Self, NorylumError> {
        let conn = rusqlite::Connection::open_in_memory().map_err(|e| {
            NorylumError::Internal(format!(
                "Impossible d'ouvrir la base SQLite en mémoire: {}",
                e
            ))
        })?;

        Self::init_connection(conn)
    }

    fn init_connection(conn: rusqlite::Connection) -> Result<Self, NorylumError> {
        // Activation du mode WAL et enforcement des clés étrangères
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA synchronous = NORMAL;",
        )
        .map_err(|e| {
            NorylumError::Internal(format!(
                "Erreur lors de la configuration des PRAGMAs SQLite: {}",
                e
            ))
        })?;

        // 1. Tables initiales (compatibles avec le schéma historique)
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS media_items (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                file_path TEXT NOT NULL UNIQUE,
                relative_path TEXT NOT NULL UNIQUE,
                file_size INTEGER NOT NULL,
                duration_seconds REAL NOT NULL,
                mime_type TEXT NOT NULL,
                video_codec TEXT,
                audio_codec TEXT,
                width INTEGER,
                height INTEGER,
                is_child_safe BOOLEAN NOT NULL DEFAULT 1,
                age_rating INTEGER,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_media_items_relative_path ON media_items(relative_path);

            CREATE TABLE IF NOT EXISTS playback_progress (
                account_id TEXT NOT NULL,
                profile_id TEXT NOT NULL,
                media_id TEXT NOT NULL,
                position_seconds REAL NOT NULL,
                duration_seconds REAL NOT NULL,
                is_completed BOOLEAN NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (account_id, profile_id, media_id)
            );

            CREATE INDEX IF NOT EXISTS idx_playback_progress_profile ON playback_progress(profile_id);",
        )
        .map_err(|e| {
            NorylumError::Internal(format!(
                "Erreur lors de l'exécution des tables initiales SQLite: {}",
                e
            ))
        })?;

        // 2. Migration additive : ajout de la colonne media_type à media_items si absente
        let has_media_type = {
            let mut stmt = conn
                .prepare("PRAGMA table_info(media_items)")
                .map_err(|e| NorylumError::Internal(e.to_string()))?;
            let mut rows = stmt
                .query([])
                .map_err(|e| NorylumError::Internal(e.to_string()))?;
            let mut found = false;
            while let Some(row) = rows
                .next()
                .map_err(|e| NorylumError::Internal(e.to_string()))?
            {
                let name: String = row
                    .get(1)
                    .map_err(|e| NorylumError::Internal(e.to_string()))?;
                if name == "media_type" {
                    found = true;
                    break;
                }
            }
            found
        };

        if !has_media_type {
            conn.execute_batch(
                "ALTER TABLE media_items ADD COLUMN media_type TEXT NOT NULL DEFAULT 'movie';",
            )
            .map_err(|e| {
                NorylumError::Internal(format!(
                    "Erreur lors de l'ajout de la colonne media_type: {}",
                    e
                ))
            })?;
        }

        // 3. Tables relationnelles pour les séries, saisons et épisodes
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS shows (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                year INTEGER,
                relative_path TEXT NOT NULL UNIQUE,
                is_child_safe BOOLEAN NOT NULL DEFAULT 1,
                age_rating INTEGER,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS seasons (
                id TEXT PRIMARY KEY,
                show_id TEXT NOT NULL REFERENCES shows(id) ON DELETE CASCADE,
                season_number INTEGER NOT NULL,
                title TEXT NOT NULL,
                relative_path TEXT NOT NULL UNIQUE,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE(show_id, season_number)
            );

            CREATE TABLE IF NOT EXISTS episodes (
                id TEXT PRIMARY KEY,
                season_id TEXT NOT NULL REFERENCES seasons(id) ON DELETE CASCADE,
                show_id TEXT NOT NULL REFERENCES shows(id) ON DELETE CASCADE,
                media_id TEXT NOT NULL UNIQUE REFERENCES media_items(id) ON DELETE CASCADE,
                episode_number INTEGER NOT NULL,
                episode_end INTEGER,
                title TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE(season_id, episode_number)
            );

            CREATE INDEX IF NOT EXISTS idx_seasons_show_id ON seasons(show_id);
            CREATE INDEX IF NOT EXISTS idx_episodes_season_id ON episodes(season_id);
            CREATE INDEX IF NOT EXISTS idx_episodes_show_id ON episodes(show_id);
            CREATE INDEX IF NOT EXISTS idx_episodes_media_id ON episodes(media_id);
            CREATE INDEX IF NOT EXISTS idx_media_items_media_type ON media_items(media_type);",
        )
        .map_err(|e| {
            NorylumError::Internal(format!(
                "Erreur lors de l'exécution des migrations de séries: {}",
                e
            ))
        })?;

        tracing::info!(
            "Base SQLite Norylum initialisée avec succès (WAL activé, migrations appliquées)"
        );

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Synchronise l'ensemble des médias scannés dans SQLite.
    /// Préserve scrupuleusement les IDs stables (`med_...`) déjà attribués aux fichiers existants,
    /// classifie les médias (`movie`, `episode`, `unknown`), met à jour les séries/saisons/épisodes,
    /// et purge les orphelins.
    pub async fn sync_media_items(
        &self,
        scanned_items: &[MediaItem],
    ) -> Result<Vec<MediaItem>, NorylumError> {
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction()
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        let now = Utc::now().to_rfc3339();
        let mut synced_items = Vec::with_capacity(scanned_items.len());

        for item in scanned_items {
            // 1. Recherche d'un ID existant pour ce chemin relatif
            let existing_id: Option<String> = tx
                .query_row(
                    "SELECT id FROM media_items WHERE relative_path = ?1",
                    params![item.relative_path],
                    |row| row.get(0),
                )
                .ok();

            // Si le média existe déjà, conserver son ID stable originel
            let stable_id = existing_id
                .map(MediaId::new)
                .unwrap_or_else(|| item.id.clone());

            let mut final_item = item.clone();
            final_item.id = stable_id;

            let file_path_str = final_item.file_path.to_string_lossy().to_string();
            let is_child_safe_int = if final_item.is_child_safe { 1 } else { 0 };
            let parsed = norylum_metadata::parse_relative_path(&final_item.relative_path);
            let media_type_str = match &parsed {
                norylum_metadata::ParsedName::Movie { .. } => "movie",
                norylum_metadata::ParsedName::Episode { .. } => "episode",
                norylum_metadata::ParsedName::Unknown => "unknown",
            };

            tx.execute(
                "INSERT INTO media_items (
                    id, title, file_path, relative_path, file_size, duration_seconds,
                    mime_type, video_codec, audio_codec, width, height, is_child_safe,
                    age_rating, created_at, updated_at, media_type
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                ON CONFLICT(relative_path) DO UPDATE SET
                    title = excluded.title,
                    file_path = excluded.file_path,
                    file_size = excluded.file_size,
                    duration_seconds = excluded.duration_seconds,
                    mime_type = excluded.mime_type,
                    video_codec = excluded.video_codec,
                    audio_codec = excluded.audio_codec,
                    width = excluded.width,
                    height = excluded.height,
                    is_child_safe = excluded.is_child_safe,
                    age_rating = excluded.age_rating,
                    updated_at = excluded.updated_at,
                    media_type = excluded.media_type;",
                params![
                    final_item.id.as_str(),
                    final_item.title,
                    file_path_str,
                    final_item.relative_path,
                    final_item.file_size as i64,
                    final_item.duration_seconds,
                    final_item.mime_type,
                    final_item.video_codec,
                    final_item.audio_codec,
                    final_item.width,
                    final_item.height,
                    is_child_safe_int,
                    final_item.age_rating,
                    now,
                    now,
                    media_type_str,
                ],
            )
            .map_err(|e| NorylumError::Internal(format!("Erreur lors de l'upsert média: {}", e)))?;

            // 2. Si c'est un épisode, insérer/mettre à jour la série, la saison et l'épisode
            if let norylum_metadata::ParsedName::Episode {
                show_title,
                season,
                episode,
                episode_end,
                year,
            } = parsed
            {
                let rel_clean = final_item.relative_path.replace('\\', "/");
                let parts: Vec<&str> = rel_clean.split('/').filter(|s| !s.is_empty()).collect();
                let show_rel_path = if parts.len() > 1 {
                    parts[0].to_string()
                } else {
                    show_title.clone()
                };

                let show_id = ShowId::from_path(&show_rel_path);
                let season_id = SeasonId::from_show_and_season(&show_id, season);
                let episode_id = EpisodeId::from_season_and_episode(&season_id, episode);

                // A. Upsert du Show
                tx.execute(
                        "INSERT INTO shows (
                            id, title, year, relative_path, is_child_safe, age_rating, created_at, updated_at
                        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                        ON CONFLICT(relative_path) DO UPDATE SET
                            title = excluded.title,
                            year = excluded.year,
                            updated_at = excluded.updated_at;",
                        params![
                            show_id.as_str(),
                            show_title,
                            year.map(|y| y as i64),
                            show_rel_path,
                            is_child_safe_int,
                            final_item.age_rating,
                            now,
                            now,
                        ],
                    )
                    .map_err(|e| NorylumError::Internal(format!("Erreur lors de l'upsert show: {}", e)))?;

                // B. Upsert de la Saison
                let season_title = if season == 0 {
                    "Hors-série".to_string()
                } else {
                    format!("Saison {:02}", season)
                };
                let season_rel_path = format!("{}/season_{}", show_rel_path, season);

                tx.execute(
                    "INSERT INTO seasons (
                            id, show_id, season_number, title, relative_path, created_at, updated_at
                        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                        ON CONFLICT(show_id, season_number) DO UPDATE SET
                            title = excluded.title,
                            relative_path = excluded.relative_path,
                            updated_at = excluded.updated_at;",
                    params![
                        season_id.as_str(),
                        show_id.as_str(),
                        season,
                        season_title,
                        season_rel_path,
                        now,
                        now,
                    ],
                )
                .map_err(|e| {
                    NorylumError::Internal(format!("Erreur lors de l'upsert season: {}", e))
                })?;

                // C. Upsert de l'Épisode
                let ep_title = norylum_metadata::extract_episode_title(&final_item.title, episode);

                tx.execute(
                        "INSERT INTO episodes (
                            id, season_id, show_id, media_id, episode_number, episode_end, title, created_at, updated_at
                        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                        ON CONFLICT(season_id, episode_number) DO UPDATE SET
                            show_id = excluded.show_id,
                            media_id = excluded.media_id,
                            episode_end = excluded.episode_end,
                            title = excluded.title,
                            updated_at = excluded.updated_at;",
                        params![
                            episode_id.as_str(),
                            season_id.as_str(),
                            show_id.as_str(),
                            final_item.id.as_str(),
                            episode,
                            episode_end.map(|e| e as i64),
                            ep_title,
                            now,
                            now,
                        ],
                    )
                    .map_err(|e| NorylumError::Internal(format!("Erreur lors de l'upsert episode: {}", e)))?;
            }

            synced_items.push(final_item);
        }

        // 3. Nettoyage des médias qui ne sont plus présents sur le disque
        if !synced_items.is_empty() {
            let relative_paths: Vec<&str> = synced_items
                .iter()
                .map(|i| i.relative_path.as_str())
                .collect();

            let placeholders = relative_paths
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");

            let delete_query = format!(
                "DELETE FROM media_items WHERE relative_path NOT IN ({})",
                placeholders
            );

            let mut stmt = tx
                .prepare(&delete_query)
                .map_err(|e| NorylumError::Internal(e.to_string()))?;

            let rusqlite_params = rusqlite::params_from_iter(relative_paths);
            stmt.execute(rusqlite_params)
                .map_err(|e| NorylumError::Internal(e.to_string()))?;
        } else {
            tx.execute("DELETE FROM media_items", [])
                .map_err(|e| NorylumError::Internal(e.to_string()))?;
        }

        // 4. Mettre à jour is_child_safe et age_rating globaux sur chaque série à partir de ses épisodes
        tx.execute(
            "UPDATE shows SET
                is_child_safe = COALESCE((
                    SELECT MIN(mi.is_child_safe)
                    FROM episodes ep
                    JOIN media_items mi ON ep.media_id = mi.id
                    WHERE ep.show_id = shows.id
                ), 1),
                age_rating = (
                    SELECT MAX(mi.age_rating)
                    FROM episodes ep
                    JOIN media_items mi ON ep.media_id = mi.id
                    WHERE ep.show_id = shows.id
                );",
            [],
        )
        .map_err(|e| NorylumError::Internal(format!("Erreur calcul sécurité séries: {}", e)))?;

        // 5. Purger les saisons orphelines (sans aucun épisode) et les séries orphelines (sans saisons)
        tx.execute(
            "DELETE FROM seasons WHERE id NOT IN (SELECT DISTINCT season_id FROM episodes);",
            [],
        )
        .map_err(|e| NorylumError::Internal(e.to_string()))?;

        tx.execute(
            "DELETE FROM shows WHERE id NOT IN (SELECT DISTINCT show_id FROM seasons);",
            [],
        )
        .map_err(|e| NorylumError::Internal(e.to_string()))?;

        tx.commit()
            .map_err(|e| NorylumError::Internal(format!("Échec commit transaction: {}", e)))?;

        Ok(synced_items)
    }

    /// Récupère un élément média par son identifiant unique.
    pub async fn get_media_by_id(&self, id: &MediaId) -> Result<Option<MediaItem>, NorylumError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, title, file_path, relative_path, file_size, duration_seconds,
                        mime_type, video_codec, audio_codec, width, height, is_child_safe, age_rating, media_type
                 FROM media_items WHERE id = ?1",
            )
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        let item = stmt
            .query_row(params![id.as_str()], Self::map_media_row)
            .ok();

        Ok(item)
    }

    /// Liste l'ensemble des médias présents dans la base SQLite.
    pub async fn list_all_media(&self) -> Result<Vec<MediaItem>, NorylumError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, title, file_path, relative_path, file_size, duration_seconds,
                        mime_type, video_codec, audio_codec, width, height, is_child_safe, age_rating, media_type
                 FROM media_items ORDER BY title ASC",
            )
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        let rows = stmt
            .query_map([], Self::map_media_row)
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        let mut items = Vec::new();
        for row in rows {
            let item = row.map_err(|e| NorylumError::Internal(e.to_string()))?;
            items.push(item);
        }

        Ok(items)
    }

    /// Liste uniquement les films enregistrés dans la base SQLite.
    pub async fn list_all_movies(&self) -> Result<Vec<MediaItem>, NorylumError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, title, file_path, relative_path, file_size, duration_seconds,
                        mime_type, video_codec, audio_codec, width, height, is_child_safe, age_rating, media_type
                 FROM media_items WHERE media_type = 'movie' ORDER BY title ASC",
            )
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        let rows = stmt
            .query_map([], Self::map_media_row)
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        let mut items = Vec::new();
        for row in rows {
            let item = row.map_err(|e| NorylumError::Internal(e.to_string()))?;
            items.push(item);
        }

        Ok(items)
    }

    /// Liste toutes les séries avec leurs compteurs de saisons et d'épisodes.
    pub async fn list_all_shows(&self) -> Result<Vec<ShowRecord>, NorylumError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT s.id, s.title, s.year, s.relative_path, s.is_child_safe, s.age_rating,
                        COUNT(DISTINCT sea.id) as season_count,
                        COUNT(DISTINCT ep.id) as episode_count,
                        s.created_at, s.updated_at
                 FROM shows s
                 LEFT JOIN seasons sea ON s.id = sea.show_id
                 LEFT JOIN episodes ep ON s.id = ep.show_id
                 GROUP BY s.id
                 ORDER BY s.title ASC",
            )
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                let id_str: String = row.get(0)?;
                let title: String = row.get(1)?;
                let year: Option<i64> = row.get(2)?;
                let relative_path: String = row.get(3)?;
                let is_child_safe_int: i32 = row.get(4)?;
                let age_rating: Option<u32> = row.get(5)?;
                let season_count: i64 = row.get(6)?;
                let episode_count: i64 = row.get(7)?;
                let created_at: String = row.get(8)?;
                let updated_at: String = row.get(9)?;

                Ok(ShowRecord {
                    id: ShowId::new(id_str),
                    title,
                    year: year.map(|y| y as u16),
                    relative_path,
                    is_child_safe: is_child_safe_int != 0,
                    age_rating,
                    season_count: season_count as u32,
                    episode_count: episode_count as u32,
                    created_at,
                    updated_at,
                })
            })
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        let mut shows = Vec::new();
        for row in rows {
            shows.push(row.map_err(|e| NorylumError::Internal(e.to_string()))?);
        }

        Ok(shows)
    }

    /// Récupère une série par son identifiant unique.
    pub async fn get_show_by_id(&self, id: &ShowId) -> Result<Option<ShowRecord>, NorylumError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT s.id, s.title, s.year, s.relative_path, s.is_child_safe, s.age_rating,
                        COUNT(DISTINCT sea.id) as season_count,
                        COUNT(DISTINCT ep.id) as episode_count,
                        s.created_at, s.updated_at
                 FROM shows s
                 LEFT JOIN seasons sea ON s.id = sea.show_id
                 LEFT JOIN episodes ep ON s.id = ep.show_id
                 WHERE s.id = ?1
                 GROUP BY s.id",
            )
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        let record = stmt
            .query_row(params![id.as_str()], |row| {
                let id_str: String = row.get(0)?;
                let title: String = row.get(1)?;
                let year: Option<i64> = row.get(2)?;
                let relative_path: String = row.get(3)?;
                let is_child_safe_int: i32 = row.get(4)?;
                let age_rating: Option<u32> = row.get(5)?;
                let season_count: i64 = row.get(6)?;
                let episode_count: i64 = row.get(7)?;
                let created_at: String = row.get(8)?;
                let updated_at: String = row.get(9)?;

                Ok(ShowRecord {
                    id: ShowId::new(id_str),
                    title,
                    year: year.map(|y| y as u16),
                    relative_path,
                    is_child_safe: is_child_safe_int != 0,
                    age_rating,
                    season_count: season_count as u32,
                    episode_count: episode_count as u32,
                    created_at,
                    updated_at,
                })
            })
            .ok();

        Ok(record)
    }

    /// Liste l'ensemble des saisons d'une série.
    pub async fn list_seasons_for_show(
        &self,
        show_id: &ShowId,
    ) -> Result<Vec<SeasonRecord>, NorylumError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT sea.id, sea.show_id, sea.season_number, sea.title, sea.relative_path,
                        COUNT(ep.id) as episode_count,
                        sea.created_at, sea.updated_at
                 FROM seasons sea
                 LEFT JOIN episodes ep ON sea.id = ep.season_id
                 WHERE sea.show_id = ?1
                 GROUP BY sea.id
                 ORDER BY sea.season_number ASC",
            )
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        let rows = stmt
            .query_map(params![show_id.as_str()], |row| {
                let id_str: String = row.get(0)?;
                let show_id_str: String = row.get(1)?;
                let season_number: i64 = row.get(2)?;
                let title: String = row.get(3)?;
                let relative_path: String = row.get(4)?;
                let episode_count: i64 = row.get(5)?;
                let created_at: String = row.get(6)?;
                let updated_at: String = row.get(7)?;

                Ok(SeasonRecord {
                    id: SeasonId::new(id_str),
                    show_id: ShowId::new(show_id_str),
                    season_number: season_number as u32,
                    title,
                    relative_path,
                    episode_count: episode_count as u32,
                    created_at,
                    updated_at,
                })
            })
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        let mut seasons = Vec::new();
        for row in rows {
            seasons.push(row.map_err(|e| NorylumError::Internal(e.to_string()))?);
        }

        Ok(seasons)
    }

    /// Récupère une saison par son numéro au sein d'une série.
    pub async fn get_season_by_number(
        &self,
        show_id: &ShowId,
        season_number: u32,
    ) -> Result<Option<SeasonRecord>, NorylumError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT sea.id, sea.show_id, sea.season_number, sea.title, sea.relative_path,
                        COUNT(ep.id) as episode_count,
                        sea.created_at, sea.updated_at
                 FROM seasons sea
                 LEFT JOIN episodes ep ON sea.id = ep.season_id
                 WHERE sea.show_id = ?1 AND sea.season_number = ?2
                 GROUP BY sea.id",
            )
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        let record = stmt
            .query_row(params![show_id.as_str(), season_number], |row| {
                let id_str: String = row.get(0)?;
                let show_id_str: String = row.get(1)?;
                let s_num: i64 = row.get(2)?;
                let title: String = row.get(3)?;
                let relative_path: String = row.get(4)?;
                let episode_count: i64 = row.get(5)?;
                let created_at: String = row.get(6)?;
                let updated_at: String = row.get(7)?;

                Ok(SeasonRecord {
                    id: SeasonId::new(id_str),
                    show_id: ShowId::new(show_id_str),
                    season_number: s_num as u32,
                    title,
                    relative_path,
                    episode_count: episode_count as u32,
                    created_at,
                    updated_at,
                })
            })
            .ok();

        Ok(record)
    }

    /// Liste l'ensemble des épisodes associés à une saison.
    pub async fn list_episodes_for_season(
        &self,
        season_id: &SeasonId,
    ) -> Result<Vec<EpisodeRecord>, NorylumError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT ep.id, ep.season_id, ep.show_id, ep.media_id, ep.episode_number, ep.episode_end,
                        ep.title, mi.duration_seconds, mi.is_child_safe, mi.age_rating,
                        ep.created_at, ep.updated_at
                 FROM episodes ep
                 JOIN media_items mi ON ep.media_id = mi.id
                 WHERE ep.season_id = ?1
                 ORDER BY ep.episode_number ASC",
            )
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        let rows = stmt
            .query_map(params![season_id.as_str()], |row| {
                let id_str: String = row.get(0)?;
                let season_id_str: String = row.get(1)?;
                let show_id_str: String = row.get(2)?;
                let media_id_str: String = row.get(3)?;
                let episode_number: i64 = row.get(4)?;
                let episode_end: Option<i64> = row.get(5)?;
                let title: String = row.get(6)?;
                let duration_seconds: f64 = row.get(7)?;
                let is_child_safe_int: i32 = row.get(8)?;
                let age_rating: Option<u32> = row.get(9)?;
                let created_at: String = row.get(10)?;
                let updated_at: String = row.get(11)?;

                Ok(EpisodeRecord {
                    id: EpisodeId::new(id_str),
                    season_id: SeasonId::new(season_id_str),
                    show_id: ShowId::new(show_id_str),
                    media_id: MediaId::new(media_id_str),
                    episode_number: episode_number as u32,
                    episode_end: episode_end.map(|e| e as u32),
                    title,
                    duration_seconds,
                    is_child_safe: is_child_safe_int != 0,
                    age_rating,
                    created_at,
                    updated_at,
                })
            })
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        let mut episodes = Vec::new();
        for row in rows {
            episodes.push(row.map_err(|e| NorylumError::Internal(e.to_string()))?);
        }

        Ok(episodes)
    }

    /// Compte le nombre total de médias enregistrés.
    pub async fn count_media(&self) -> Result<usize, NorylumError> {
        let conn = self.conn.lock().await;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM media_items", [], |r| r.get(0))
            .map_err(|e| NorylumError::Internal(e.to_string()))?;
        Ok(count as usize)
    }

    fn map_media_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MediaItem> {
        let id_str: String = row.get(0)?;
        let title: String = row.get(1)?;
        let file_path_str: String = row.get(2)?;
        let relative_path: String = row.get(3)?;
        let file_size: i64 = row.get(4)?;
        let duration_seconds: f64 = row.get(5)?;
        let mime_type: String = row.get(6)?;
        let video_codec: Option<String> = row.get(7)?;
        let audio_codec: Option<String> = row.get(8)?;
        let width: Option<u32> = row.get(9)?;
        let height: Option<u32> = row.get(10)?;
        let is_child_safe_int: i32 = row.get(11)?;
        let age_rating: Option<u32> = row.get(12)?;

        Ok(MediaItem {
            id: MediaId::new(id_str),
            title,
            file_path: PathBuf::from(file_path_str),
            relative_path,
            file_size: file_size as u64,
            duration_seconds,
            mime_type,
            video_codec,
            audio_codec,
            width,
            height,
            is_child_safe: is_child_safe_int != 0,
            age_rating,
        })
    }

    /// Enregistre ou met à jour la progression de lecture pour un profil et un média.
    pub async fn save_playback_progress(
        &self,
        account_id: &AccountId,
        profile_id: &ProfileId,
        media_id: &MediaId,
        position_seconds: f64,
        duration_seconds: f64,
    ) -> Result<PlaybackProgressRecord, NorylumError> {
        let is_completed = if duration_seconds > 0.0 {
            (position_seconds / duration_seconds) >= 0.90
        } else {
            false
        };

        let now = Utc::now();
        let now_str = now.to_rfc3339();

        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO playback_progress (
                account_id, profile_id, media_id, position_seconds, duration_seconds, is_completed, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT (account_id, profile_id, media_id) DO UPDATE SET
                position_seconds = excluded.position_seconds,
                duration_seconds = excluded.duration_seconds,
                is_completed = excluded.is_completed,
                updated_at = excluded.updated_at;",
            params![
                account_id.as_str(),
                profile_id.as_str(),
                media_id.as_str(),
                position_seconds,
                duration_seconds,
                if is_completed { 1 } else { 0 },
                now_str,
            ],
        )
        .map_err(|e| NorylumError::Internal(format!("Erreur lors de la sauvegarde de progression: {}", e)))?;

        Ok(PlaybackProgressRecord {
            account_id: account_id.clone(),
            profile_id: profile_id.clone(),
            media_id: media_id.clone(),
            position_seconds,
            duration_seconds,
            is_completed,
            updated_at: now,
        })
    }

    /// Récupère la progression de lecture d'un média pour un profil donné.
    pub async fn get_playback_progress(
        &self,
        account_id: &AccountId,
        profile_id: &ProfileId,
        media_id: &MediaId,
    ) -> Result<Option<PlaybackProgressRecord>, NorylumError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT position_seconds, duration_seconds, is_completed, updated_at
                 FROM playback_progress
                 WHERE account_id = ?1 AND profile_id = ?2 AND media_id = ?3",
            )
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        let record = stmt
            .query_row(
                params![account_id.as_str(), profile_id.as_str(), media_id.as_str()],
                |row| {
                    let pos: f64 = row.get(0)?;
                    let dur: f64 = row.get(1)?;
                    let comp: i32 = row.get(2)?;
                    let updated_str: String = row.get(3)?;
                    let dt = DateTime::parse_from_rfc3339(&updated_str)
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now());

                    Ok(PlaybackProgressRecord {
                        account_id: account_id.clone(),
                        profile_id: profile_id.clone(),
                        media_id: media_id.clone(),
                        position_seconds: pos,
                        duration_seconds: dur,
                        is_completed: comp != 0,
                        updated_at: dt,
                    })
                },
            )
            .ok();

        Ok(record)
    }

    /// Liste toutes les progressions enregistrées pour un profil.
    pub async fn list_playback_progress_for_profile(
        &self,
        account_id: &AccountId,
        profile_id: &ProfileId,
    ) -> Result<Vec<PlaybackProgressRecord>, NorylumError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT media_id, position_seconds, duration_seconds, is_completed, updated_at
                 FROM playback_progress
                 WHERE account_id = ?1 AND profile_id = ?2",
            )
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        let rows = stmt
            .query_map(params![account_id.as_str(), profile_id.as_str()], |row| {
                let m_id: String = row.get(0)?;
                let pos: f64 = row.get(1)?;
                let dur: f64 = row.get(2)?;
                let comp: i32 = row.get(3)?;
                let updated_str: String = row.get(4)?;
                let dt = DateTime::parse_from_rfc3339(&updated_str)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());

                Ok(PlaybackProgressRecord {
                    account_id: account_id.clone(),
                    profile_id: profile_id.clone(),
                    media_id: MediaId::new(m_id),
                    position_seconds: pos,
                    duration_seconds: dur,
                    is_completed: comp != 0,
                    updated_at: dt,
                })
            })
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        let mut list = Vec::new();
        for r in rows {
            list.push(r.map_err(|e| NorylumError::Internal(e.to_string()))?);
        }

        Ok(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sqlite_wal_and_migrations() {
        let db = Database::in_memory().unwrap();
        let conn = db.conn.lock().await;

        // Vérification des PRAGMAs
        let fk: i32 = conn
            .query_row("PRAGMA foreign_keys;", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);

        // Vérification de la création des tables
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('media_items', 'playback_progress', 'shows', 'seasons', 'episodes');",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 5);
    }

    #[tokio::test]
    async fn test_media_persistence_and_stable_ids() {
        let db = Database::in_memory().unwrap();

        let initial_item = MediaItem {
            id: MediaId::new("med_origin_123"),
            title: "Super Lapin".into(),
            file_path: PathBuf::from("/media/lapin.mp4"),
            relative_path: "lapin.mp4".into(),
            file_size: 1000,
            duration_seconds: 120.0,
            mime_type: "video/mp4".into(),
            video_codec: Some("h264".into()),
            audio_codec: Some("aac".into()),
            width: Some(1920),
            height: Some(1080),
            is_child_safe: true,
            age_rating: Some(0),
        };

        // 1. Première synchronisation
        let synced = db
            .sync_media_items(std::slice::from_ref(&initial_item))
            .await
            .unwrap();
        assert_eq!(synced.len(), 1);
        assert_eq!(synced[0].id.as_str(), "med_origin_123");

        // 2. Deuxième synchronisation avec un ID temporaire différent
        let mut rescanned_item = initial_item.clone();
        rescanned_item.id = MediaId::new("med_new_calc_456");
        rescanned_item.file_size = 1050;

        let resynced = db.sync_media_items(&[rescanned_item]).await.unwrap();
        assert_eq!(resynced.len(), 1);
        assert_eq!(resynced[0].id.as_str(), "med_origin_123");
        assert_eq!(resynced[0].file_size, 1050);

        // 3. Vérification de la lecture
        let read_back = db
            .get_media_by_id(&MediaId::new("med_origin_123"))
            .await
            .unwrap();
        assert!(read_back.is_some());
        assert_eq!(read_back.unwrap().title, "Super Lapin");
    }

    #[tokio::test]
    async fn test_playback_progress_persistence() {
        let db = Database::in_memory().unwrap();

        let acc = AccountId::new("acc_famille");
        let prof = ProfileId::new("prof_enfant");
        let media = MediaId::new("med_dessin");

        let prog1 = db
            .save_playback_progress(&acc, &prof, &media, 50.0, 100.0)
            .await
            .unwrap();
        assert!(!prog1.is_completed);
        assert_eq!(prog1.position_seconds, 50.0);

        let prog2 = db
            .save_playback_progress(&acc, &prof, &media, 92.0, 100.0)
            .await
            .unwrap();
        assert!(prog2.is_completed);
        assert_eq!(prog2.position_seconds, 92.0);

        let fetched = db.get_playback_progress(&acc, &prof, &media).await.unwrap();
        assert!(fetched.is_some());
        let f = fetched.unwrap();
        assert_eq!(f.position_seconds, 92.0);
        assert!(f.is_completed);
    }

    #[tokio::test]
    async fn test_legacy_database_migration() {
        // Test de non-régression exigé : une base créée avec l'ANCIEN schéma doit migrer
        // sans perte, en conservant media_id et playback_progress intacts.
        let temp_dir = std::env::temp_dir().join(format!("norylum_mig_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let db_path = temp_dir.join("legacy_norylum.sqlite3");

        // 1. Création directe avec l'ANCIEN schéma
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE media_items (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL,
                    file_path TEXT NOT NULL UNIQUE,
                    relative_path TEXT NOT NULL UNIQUE,
                    file_size INTEGER NOT NULL,
                    duration_seconds REAL NOT NULL,
                    mime_type TEXT NOT NULL,
                    video_codec TEXT,
                    audio_codec TEXT,
                    width INTEGER,
                    height INTEGER,
                    is_child_safe BOOLEAN NOT NULL DEFAULT 1,
                    age_rating INTEGER,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE playback_progress (
                    account_id TEXT NOT NULL,
                    profile_id TEXT NOT NULL,
                    media_id TEXT NOT NULL,
                    position_seconds REAL NOT NULL,
                    duration_seconds REAL NOT NULL,
                    is_completed BOOLEAN NOT NULL DEFAULT 0,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY (account_id, profile_id, media_id)
                );",
            )
            .unwrap();

            // Insérer un média ancien et une progression
            conn.execute(
                "INSERT INTO media_items VALUES (
                    'med_legacy_001', 'Ancien Film', '/data/film.mp4', 'film.mp4',
                    5000000, 3600.0, 'video/mp4', 'h264', 'aac', 1920, 1080, 1, 0,
                    '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'
                );",
                [],
            )
            .unwrap();

            conn.execute(
                "INSERT INTO playback_progress VALUES (
                    'acc_1', 'prof_1', 'med_legacy_001', 1800.0, 3600.0, 0, '2026-01-01T00:00:00Z'
                );",
                [],
            )
            .unwrap();
        }

        // 2. Ouverture via notre Database qui exécute les migrations automatiques
        let db = Database::open(&db_path).unwrap();

        // 3. Vérification que l'ancien média est présent et a conservé son MediaId
        let item = db
            .get_media_by_id(&MediaId::new("med_legacy_001"))
            .await
            .unwrap();
        assert!(item.is_some());
        let item = item.unwrap();
        assert_eq!(item.id.as_str(), "med_legacy_001");
        assert_eq!(item.title, "Ancien Film");

        // 4. Vérification que la progression a survécu
        let prog = db
            .get_playback_progress(
                &AccountId::new("acc_1"),
                &ProfileId::new("prof_1"),
                &MediaId::new("med_legacy_001"),
            )
            .await
            .unwrap();
        assert!(prog.is_some());
        assert_eq!(prog.unwrap().position_seconds, 1800.0);

        // 5. Nettoyage
        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn test_shows_seasons_episodes_persistence_and_cascade_delete() {
        let db = Database::in_memory().unwrap();

        // 1. Synchronisation initiale avec 1 film et 2 épisodes d'une même série
        let movie = MediaItem {
            id: MediaId::new("med_movie_1"),
            title: "Avengers - Endgame".into(),
            file_path: PathBuf::from("/media/Avengers - Endgame (2019).mkv"),
            relative_path: "Avengers - Endgame (2019).mkv".into(),
            file_size: 5000,
            duration_seconds: 7200.0,
            mime_type: "video/x-matroska".into(),
            video_codec: Some("hevc".into()),
            audio_codec: Some("ac3".into()),
            width: Some(3840),
            height: Some(2160),
            is_child_safe: true,
            age_rating: Some(12),
        };

        let ep1 = MediaItem {
            id: MediaId::new("med_ep_1"),
            title: "Bécé Bégé - S01E01 - Tel est pris".into(),
            file_path: PathBuf::from(
                "/media/Bécé Bégé/Saison 01/Bécé Bégé - S01E01 - Tel est pris.mkv",
            ),
            relative_path: "Bécé Bégé/Saison 01/Bécé Bégé - S01E01 - Tel est pris.mkv".into(),
            file_size: 1000,
            duration_seconds: 1200.0,
            mime_type: "video/x-matroska".into(),
            video_codec: Some("h264".into()),
            audio_codec: Some("aac".into()),
            width: Some(1920),
            height: Some(1080),
            is_child_safe: true,
            age_rating: Some(0),
        };

        let ep2 = MediaItem {
            id: MediaId::new("med_ep_2"),
            title: "Bécé Bégé - S01E02 - Le chien a mangé mes devoirs".into(),
            file_path: PathBuf::from(
                "/media/Bécé Bégé/Saison 01/Bécé Bégé - S01E02 - Le chien a mangé mes devoirs.mkv",
            ),
            relative_path:
                "Bécé Bégé/Saison 01/Bécé Bégé - S01E02 - Le chien a mangé mes devoirs.mkv".into(),
            file_size: 1000,
            duration_seconds: 1200.0,
            mime_type: "video/x-matroska".into(),
            video_codec: Some("h264".into()),
            audio_codec: Some("aac".into()),
            width: Some(1920),
            height: Some(1080),
            is_child_safe: true,
            age_rating: Some(0),
        };

        let items = vec![movie.clone(), ep1.clone(), ep2.clone()];
        db.sync_media_items(&items).await.unwrap();

        // 2. Vérification des listes
        let movies = db.list_all_movies().await.unwrap();
        assert_eq!(movies.len(), 1);
        assert_eq!(movies[0].title, "Avengers - Endgame");

        let shows = db.list_all_shows().await.unwrap();
        assert_eq!(shows.len(), 1);
        assert_eq!(shows[0].title, "Bécé Bégé");
        assert_eq!(shows[0].season_count, 1);
        assert_eq!(shows[0].episode_count, 2);

        let show_id = shows[0].id.clone();
        let seasons = db.list_seasons_for_show(&show_id).await.unwrap();
        assert_eq!(seasons.len(), 1);
        assert_eq!(seasons[0].season_number, 1);
        assert_eq!(seasons[0].episode_count, 2);

        let season_id = seasons[0].id.clone();
        let episodes = db.list_episodes_for_season(&season_id).await.unwrap();
        assert_eq!(episodes.len(), 2);
        assert_eq!(episodes[0].episode_number, 1);
        assert_eq!(episodes[0].title, "Tel est pris");
        assert_eq!(episodes[0].media_id, ep1.id);
        assert_eq!(episodes[1].episode_number, 2);
        assert_eq!(episodes[1].title, "Le chien a mangé mes devoirs");
        assert_eq!(episodes[1].media_id, ep2.id);

        let ep1_id = episodes[0].id.clone();

        // 3. Rescan : conservation des identifiants existants
        db.sync_media_items(&items).await.unwrap();
        let re_episodes = db.list_episodes_for_season(&season_id).await.unwrap();
        assert_eq!(re_episodes[0].id, ep1_id);

        // 4. Suppression d'un épisode (fichier ep2 disparaît)
        db.sync_media_items(&[movie.clone(), ep1.clone()])
            .await
            .unwrap(); // ep1 toujours présent
        let remaining_episodes = db.list_episodes_for_season(&season_id).await.unwrap();
        assert_eq!(remaining_episodes.len(), 1);
        assert_eq!(remaining_episodes[0].episode_number, 1);

        // 5. Suppression de tous les épisodes (ep1 disparaît aussi) -> purge saison & série
        db.sync_media_items(std::slice::from_ref(&movie))
            .await
            .unwrap();
        let empty_shows = db.list_all_shows().await.unwrap();
        assert_eq!(empty_shows.len(), 0);
        let empty_seasons = db.list_seasons_for_show(&show_id).await.unwrap();
        assert_eq!(empty_seasons.len(), 0);

        // Le film est toujours là
        let still_movies = db.list_all_movies().await.unwrap();
        assert_eq!(still_movies.len(), 1);
    }
}
