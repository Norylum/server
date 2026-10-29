//! Crate `persistence` pour Norylum Server.
//! Gère la base de données SQLite local-first en mode WAL, l'exécution automatique des migrations
//! et la persistance de l'index du catalogue et des progressions de lecture.

use chrono::{DateTime, Utc};
use norylum_foundation::{AccountId, MediaId, NorylumError, ProfileId};
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

        // Exécution des migrations initiales
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
                "Erreur lors de l'exécution des migrations SQLite: {}",
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
    /// Préserve scrupuleusement les IDs stables (`med_...`) déjà attribués aux fichiers existants.
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

            tx.execute(
                "INSERT INTO media_items (
                    id, title, file_path, relative_path, file_size, duration_seconds,
                    mime_type, video_codec, audio_codec, width, height, is_child_safe,
                    age_rating, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
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
                    updated_at = excluded.updated_at;",
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
                ],
            )
            .map_err(|e| NorylumError::Internal(format!("Erreur lors de l'upsert média: {}", e)))?;

            synced_items.push(final_item);
        }

        // 2. Nettoyage des médias qui ne sont plus présents sur le disque
        if !synced_items.is_empty() {
            // Construction des marqueurs de substitution pour la clause NOT IN
            let relative_paths: Vec<&str> = synced_items
                .iter()
                .map(|i| i.relative_path.as_str())
                .collect();

            // Pour éviter la limite de paramètres SQLite, on peut purger en mémoire ou par boucle si nécessaire
            // Dans le cas nominal, on supprime les chemins non listés
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
                        mime_type, video_codec, audio_codec, width, height, is_child_safe, age_rating
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
                        mime_type, video_codec, audio_codec, width, height, is_child_safe, age_rating
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
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('media_items', 'playback_progress');",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
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

        // 2. Deuxième synchronisation avec un ID temporaire différent (par ex. calculé par taille modifiée)
        let mut rescanned_item = initial_item.clone();
        rescanned_item.id = MediaId::new("med_new_calc_456");
        rescanned_item.file_size = 1050; // La taille a changé

        let resynced = db.sync_media_items(&[rescanned_item]).await.unwrap();
        assert_eq!(resynced.len(), 1);
        // L'ID stable initial DOIT être préservé !
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

        // Progression < 90% -> is_completed = false
        let prog1 = db
            .save_playback_progress(&acc, &prof, &media, 50.0, 100.0)
            .await
            .unwrap();
        assert!(!prog1.is_completed);
        assert_eq!(prog1.position_seconds, 50.0);

        // Progression >= 90% -> is_completed = true
        let prog2 = db
            .save_playback_progress(&acc, &prof, &media, 92.0, 100.0)
            .await
            .unwrap();
        assert!(prog2.is_completed);
        assert_eq!(prog2.position_seconds, 92.0);

        // Récupération
        let fetched = db.get_playback_progress(&acc, &prof, &media).await.unwrap();
        assert!(fetched.is_some());
        let f = fetched.unwrap();
        assert_eq!(f.position_seconds, 92.0);
        assert!(f.is_completed);
    }
}
