//! Cache local des fiches TMDB, sans identite de serveur.

use crate::Database;
use chrono::Utc;
use norylum_foundation::{MediaId, NorylumError, ShowId};
use norylum_metadata::{poster_url, TmdbItem, TmdbKind};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::Serialize;

fn storage(e: impl std::fmt::Display) -> NorylumError {
    NorylumError::Internal(format!("Erreur metadonnees SQLite : {e}"))
}

#[derive(Debug, Clone, Serialize)]
pub struct MetadataRecord {
    pub tmdb_id: u64,
    pub tmdb_ref: String,
    pub kind: TmdbKind,
    pub title: String,
    pub overview: String,
    pub poster_url: Option<String>,
    pub release_date: Option<String>,
    pub source: String,
    pub provenance: String,
    pub updated_at: String,
}

type MetadataRow = (
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
);

fn table_for(kind: TmdbKind) -> (&'static str, &'static str, &'static str) {
    match kind {
        TmdbKind::Movie => (
            "media_metadata",
            "media_id",
            "SELECT 1 FROM media_items
             WHERE id = ?1 AND media_type = 'movie'",
        ),
        TmdbKind::Tv => (
            "show_metadata",
            "show_id",
            "SELECT 1 FROM shows WHERE id = ?1",
        ),
    }
}

fn make_record(kind: TmdbKind, row: MetadataRow) -> Result<MetadataRecord, NorylumError> {
    let (id, title, overview, poster_path, release_date, updated_at, source, provenance) = row;

    let tmdb_id = u64::try_from(id).map_err(storage)?;

    Ok(MetadataRecord {
        tmdb_id,
        tmdb_ref: format!("{}/{}", kind.as_str(), tmdb_id),
        kind,
        title,
        overview,
        poster_url: poster_path.as_deref().and_then(poster_url),
        release_date,
        source,
        provenance,
        updated_at,
    })
}

impl Database {
    async fn read_metadata(
        &self,
        kind: TmdbKind,
        id: &str,
    ) -> Result<Option<MetadataRecord>, NorylumError> {
        let (table, column, _) = table_for(kind);

        self.with_connection(|conn| {
            let sql = format!(
                "SELECT tmdb_id, title, overview,
                        poster_path, release_date,
                        updated_at, source, provenance
                 FROM {table} WHERE {column} = ?1"
            );

            let row: Option<MetadataRow> = conn
                .query_row(&sql, params![id], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                })
                .optional()
                .map_err(storage)?;

            row.map(|data| make_record(kind, data)).transpose()
        })
        .await
    }

    async fn write_metadata(
        &self,
        kind: TmdbKind,
        id: &str,
        item: &TmdbItem,
    ) -> Result<MetadataRecord, NorylumError> {
        if item.kind != kind || item.tmdb_id == 0 || item.tmdb_id > i64::MAX as u64 {
            return Err(NorylumError::Validation(
                "Type ou identifiant TMDB incorrect".into(),
            ));
        }

        let (table, column, lookup) = table_for(kind);
        let updated_at = Utc::now().to_rfc3339();

        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;

            let exists: Option<i64> = tx
                .query_row(lookup, params![id], |row| row.get(0))
                .optional()
                .map_err(storage)?;

            if exists.is_none() {
                return Err(NorylumError::NotFound(
                    "Film ou serie indexe introuvable".into(),
                ));
            }

            let sql = format!(
                "INSERT INTO {table} (
                    {column}, tmdb_id, title, overview,
                    poster_path, release_date, updated_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7
                 )
                 ON CONFLICT ({column}) DO UPDATE SET
                    tmdb_id = excluded.tmdb_id,
                    title = excluded.title,
                    overview = excluded.overview,
                    poster_path = excluded.poster_path,
                    release_date = excluded.release_date,
                    updated_at = excluded.updated_at"
            );

            tx.execute(
                &sql,
                params![
                    id,
                    item.tmdb_id as i64,
                    &item.title,
                    &item.overview,
                    item.poster_path.as_deref(),
                    item.release_date.as_deref(),
                    &updated_at,
                ],
            )
            .map_err(storage)?;

            tx.commit().map_err(storage)?;

            Ok(MetadataRecord {
                tmdb_id: item.tmdb_id,
                tmdb_ref: format!("{}/{}", kind.as_str(), item.tmdb_id),
                kind,
                title: item.title.clone(),
                overview: item.overview.clone(),
                poster_url: item.poster_path.as_deref().and_then(poster_url),
                release_date: item.release_date.clone(),
                source: "TMDB".into(),
                provenance: "Norylum Server".into(),
                updated_at,
            })
        })
        .await
    }

    pub async fn get_movie_metadata(
        &self,
        media_id: &MediaId,
    ) -> Result<Option<MetadataRecord>, NorylumError> {
        self.read_metadata(TmdbKind::Movie, media_id.as_str()).await
    }

    pub async fn get_show_metadata(
        &self,
        show_id: &ShowId,
    ) -> Result<Option<MetadataRecord>, NorylumError> {
        self.read_metadata(TmdbKind::Tv, show_id.as_str()).await
    }

    pub async fn set_movie_metadata(
        &self,
        media_id: &MediaId,
        item: &TmdbItem,
    ) -> Result<MetadataRecord, NorylumError> {
        self.write_metadata(TmdbKind::Movie, media_id.as_str(), item)
            .await
    }

    pub async fn set_show_metadata(
        &self,
        show_id: &ShowId,
        item: &TmdbItem,
    ) -> Result<MetadataRecord, NorylumError> {
        self.write_metadata(TmdbKind::Tv, show_id.as_str(), item)
            .await
    }

    /// Utilise seulement par les routes principales
    /// pour la correction manuelle des associations.
    pub async fn is_principal_profile(
        &self,
        account_id: &norylum_foundation::AccountId,
        profile_id: &norylum_foundation::ProfileId,
    ) -> Result<bool, NorylumError> {
        self.with_connection(|conn| {
            let principal: Option<String> = conn
                .query_row(
                    "SELECT id FROM profiles
                     WHERE account_id = ?1
                     ORDER BY rowid ASC LIMIT 1",
                    params![account_id.as_str()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;

            Ok(principal.as_deref() == Some(profile_id.as_str()))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use norylum_library::MediaItem;
    use std::path::PathBuf;

    fn movie() -> MediaItem {
        MediaItem {
            id: MediaId::new("med_tmdb_test"),
            title: "Test Film".into(),
            file_path: PathBuf::from("/tmp/Test Film (2000).mkv"),
            relative_path: "Test Film (2000).mkv".into(),
            file_size: 500,
            duration_seconds: 100.0,
            mime_type: "video/x-matroska".into(),
            video_codec: None,
            audio_codec: None,
            width: None,
            height: None,
            is_child_safe: true,
            age_rating: Some(0),
        }
    }

    fn sample(kind: TmdbKind) -> TmdbItem {
        TmdbItem {
            tmdb_id: 603,
            tmdb_ref: format!("{}/603", kind.as_str()),
            kind,
            title: "Test TMDB".into(),
            overview: "Synopsis depuis TMDB".into(),
            poster_path: Some("/cover.jpg".into()),
            poster_url: poster_url("/cover.jpg"),
            release_date: Some("2000-01-01".into()),
        }
    }

    #[tokio::test]
    async fn test_movie_cache_survives_rescan() {
        let db = Database::in_memory().unwrap();
        let item = movie();

        db.sync_media_items(std::slice::from_ref(&item))
            .await
            .unwrap();

        let saved = db
            .set_movie_metadata(&item.id, &sample(TmdbKind::Movie))
            .await
            .unwrap();

        assert_eq!(saved.tmdb_ref, "movie/603");
        assert_eq!(saved.provenance, "Norylum Server");

        db.sync_media_items(std::slice::from_ref(&item))
            .await
            .unwrap();

        let cached = db.get_movie_metadata(&item.id).await.unwrap().unwrap();

        assert_eq!(cached.tmdb_id, 603);

        assert!(matches!(
            db.set_movie_metadata(&item.id, &sample(TmdbKind::Tv)).await,
            Err(NorylumError::Validation(_))
        ));
    }

    #[tokio::test]
    async fn test_tv_cache_and_foreign_key() {
        let db = Database::in_memory().unwrap();
        let show_id = ShowId::new("shw_tmdb_test");

        db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO shows (
                    id, title, year, relative_path,
                    is_child_safe, age_rating,
                    created_at, updated_at
                 ) VALUES (
                    'shw_tmdb_test', 'Serie Test', 2000,
                    'Serie Test', 1, 0, 'now', 'now'
                 )",
                [],
            )
            .map_err(storage)?;
            Ok(())
        })
        .await
        .unwrap();

        let saved = db
            .set_show_metadata(&show_id, &sample(TmdbKind::Tv))
            .await
            .unwrap();

        assert_eq!(saved.tmdb_ref, "tv/603");

        db.with_connection(|conn| {
            conn.execute("DELETE FROM shows WHERE id = ?1", params![show_id.as_str()])
                .map_err(storage)?;
            Ok(())
        })
        .await
        .unwrap();

        assert!(db.get_show_metadata(&show_id).await.unwrap().is_none());
    }
}
