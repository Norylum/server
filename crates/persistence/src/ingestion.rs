//! Incremental SQLite upsert and durable ingestion journal.
use crate::Database;
use chrono::Utc;
use norylum_foundation::{EpisodeId, MediaId, NorylumError, SeasonId, ShowId};
use norylum_library::MediaItem;
use norylum_metadata::{parse_relative_path, ParsedName};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::Serialize;

fn storage(e: impl std::fmt::Display) -> NorylumError {
    NorylumError::Internal(format!("SQLite ingestion : {e}"))
}

#[derive(Debug, Clone, Serialize)]
pub struct IngestionJob {
    pub source: String,
    pub destination: String,
    pub status: String,
    pub attempts: u64,
    pub media_id: Option<String>,
    pub last_error: Option<String>,
    pub updated_at: String,
}

impl Database {
    pub async fn begin_ingestion(
        &self,
        source: &str,
        destination: &str,
    ) -> Result<(), NorylumError> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            conn.execute(
                "INSERT INTO ingestion_jobs (source,destination,status,attempts,updated_at)
                 VALUES (?1,?2,'processing',1,?3)
                 ON CONFLICT(source) DO UPDATE SET destination=excluded.destination,
                   status='processing',attempts=attempts+1,last_error=NULL,updated_at=excluded.updated_at",
                params![source, destination, now],
            ).map_err(storage)?;
            Ok(())
        }).await
    }

    pub async fn finish_ingestion(
        &self,
        source: &str,
        result: Result<&MediaId, &NorylumError>,
    ) -> Result<(), NorylumError> {
        let (status, media, error) = match result {
            Ok(id) => ("published", Some(id.as_str().to_owned()), None),
            Err(e) => (
                "failed",
                None,
                Some(e.to_string().chars().take(800).collect::<String>()),
            ),
        };
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            conn.execute(
                "UPDATE ingestion_jobs SET status=?2,media_id=?3,last_error=?4,updated_at=?5 WHERE source=?1",
                params![source, status, media, error, now],
            ).map_err(storage)?;
            Ok(())
        }).await
    }

    pub async fn list_ingestion_jobs(&self) -> Result<Vec<IngestionJob>, NorylumError> {
        self.with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT source,destination,status,attempts,media_id,last_error,updated_at
                 FROM ingestion_jobs ORDER BY updated_at DESC LIMIT 100",
                )
                .map_err(storage)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(IngestionJob {
                        source: row.get(0)?,
                        destination: row.get(1)?,
                        status: row.get(2)?,
                        attempts: row.get::<_, i64>(3)?.try_into().unwrap_or(0),
                        media_id: row.get(4)?,
                        last_error: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                })
                .map_err(storage)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(storage)
        })
        .await
    }

    pub async fn ingestion_media_for_path(
        &self,
        path: &str,
    ) -> Result<Option<MediaItem>, NorylumError> {
        let id: Option<String> = self
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT id FROM media_items WHERE relative_path=?1",
                    params![path],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)
            })
            .await?;
        match id {
            Some(id) => self.get_media_by_id(&MediaId::new(id)).await,
            None => Ok(None),
        }
    }

    /// Insert/update exactly one media row and related TV hierarchy, never purge others.
    pub async fn upsert_ingested_media(&self, item: &MediaItem) -> Result<MediaItem, NorylumError> {
        let now = Utc::now().to_rfc3339();
        let size = i64::try_from(item.file_size).map_err(storage)?;
        self.with_connection(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            let existing: Option<String> = tx.query_row(
                "SELECT id FROM media_items WHERE relative_path=?1", params![item.relative_path],
                |row| row.get(0),
            ).optional().map_err(storage)?;
            let mut saved = item.clone();
            if let Some(id) = existing { saved.id = MediaId::new(id); }
            let parsed = parse_relative_path(&saved.relative_path);
            let media_type = match &parsed {
                ParsedName::Movie { .. } => "movie",
                ParsedName::Episode { .. } => "episode",
                ParsedName::Unknown => "unknown",
            };
            tx.execute(
                "INSERT INTO media_items
                 (id,title,file_path,relative_path,file_size,duration_seconds,mime_type,
                  video_codec,audio_codec,width,height,is_child_safe,age_rating,created_at,updated_at,media_type)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
                 ON CONFLICT(relative_path) DO UPDATE SET
                 title=excluded.title,file_path=excluded.file_path,file_size=excluded.file_size,
                 duration_seconds=excluded.duration_seconds,mime_type=excluded.mime_type,
                 video_codec=excluded.video_codec,audio_codec=excluded.audio_codec,
                 width=excluded.width,height=excluded.height,is_child_safe=excluded.is_child_safe,
                 age_rating=excluded.age_rating,updated_at=excluded.updated_at,media_type=excluded.media_type",
                params![
                    saved.id.as_str(), &saved.title, saved.file_path.to_string_lossy().as_ref(),
                    &saved.relative_path, size, saved.duration_seconds, &saved.mime_type,
                    &saved.video_codec, &saved.audio_codec, saved.width, saved.height,
                    i32::from(saved.is_child_safe), saved.age_rating, &now, &now, media_type,
                ],
            ).map_err(storage)?;
            if let ParsedName::Episode { show_title, season, episode, episode_end, year } = parsed {
                let rel = saved.relative_path.replace('\\', "/");
                let parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
                let show_path = if parts.len() > 1 { parts[0].to_owned() } else { show_title.clone() };
                let show_id = ShowId::from_path(&show_path);
                let season_id = SeasonId::from_show_and_season(&show_id, season);
                let episode_id = EpisodeId::from_season_and_episode(&season_id, episode);
                tx.execute(
                    "INSERT INTO shows (id,title,year,relative_path,is_child_safe,age_rating,created_at,updated_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
                     ON CONFLICT(relative_path) DO UPDATE SET title=excluded.title,year=excluded.year,
                     updated_at=excluded.updated_at",
                    params![show_id.as_str(), show_title, year.map(i64::from), &show_path,
                        i32::from(saved.is_child_safe), saved.age_rating, &now, &now],
                ).map_err(storage)?;
                let season_title = if season == 0 { "Hors-serie".to_owned() } else { format!("Saison {season:02}") };
                tx.execute(
                    "INSERT INTO seasons (id,show_id,season_number,title,relative_path,created_at,updated_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7)
                     ON CONFLICT(show_id,season_number) DO UPDATE SET title=excluded.title,
                     relative_path=excluded.relative_path,updated_at=excluded.updated_at",
                    params![season_id.as_str(), show_id.as_str(), season, season_title,
                        format!("{show_path}/season_{season}"), &now, &now],
                ).map_err(storage)?;
                let episode_title = norylum_metadata::extract_episode_title(&saved.title, episode);
                tx.execute(
                    "INSERT INTO episodes
                     (id,season_id,show_id,media_id,episode_number,episode_end,title,created_at,updated_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
                     ON CONFLICT(season_id,episode_number) DO UPDATE SET show_id=excluded.show_id,
                     media_id=excluded.media_id,episode_end=excluded.episode_end,
                     title=excluded.title,updated_at=excluded.updated_at",
                    params![episode_id.as_str(), season_id.as_str(), show_id.as_str(), saved.id.as_str(),
                        episode, episode_end.map(i64::from), episode_title, &now, &now],
                ).map_err(storage)?;
            }
            tx.commit().map_err(storage)?;
            Ok(saved)
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    fn media(path: &str) -> MediaItem {
        MediaItem {
            id: norylum_library::generate_deterministic_media_id(path, 99),
            title: "Test".into(),
            file_path: PathBuf::from(format!("/tmp/{path}")),
            relative_path: path.into(),
            file_size: 99,
            duration_seconds: 2.0,
            mime_type: "video/x-matroska".into(),
            video_codec: Some("mpeg4".into()),
            audio_codec: None,
            width: Some(320),
            height: Some(180),
            is_child_safe: true,
            age_rating: Some(0),
        }
    }
    #[tokio::test]
    async fn upsert_does_not_purge_older_media() {
        let db = Database::in_memory().unwrap();
        let a = media("A (2020).mkv");
        let b = media("B (2021).mkv");
        db.upsert_ingested_media(&a).await.unwrap();
        db.upsert_ingested_media(&b).await.unwrap();
        assert_eq!(db.list_all_media().await.unwrap().len(), 2);
        let again = db.upsert_ingested_media(&b).await.unwrap();
        assert_eq!(again.id, b.id);
        assert_eq!(db.list_all_media().await.unwrap().len(), 2);
    }
    #[tokio::test]
    async fn journal_tracks_retries() {
        let db = Database::in_memory().unwrap();
        db.begin_ingestion("A.mkv", "A.mkv").await.unwrap();
        db.finish_ingestion("A.mkv", Err(&NorylumError::Conflict("Not ready".into())))
            .await
            .unwrap();
        db.begin_ingestion("A.mkv", "A.mkv").await.unwrap();
        let id = MediaId::new("med_test");
        db.finish_ingestion("A.mkv", Ok(&id)).await.unwrap();
        let jobs = db.list_ingestion_jobs().await.unwrap();
        assert_eq!(jobs[0].attempts, 2);
        assert_eq!(jobs[0].status, "published");
    }
}
