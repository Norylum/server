//! Durable opt-in torrent control, independent of the seeder process.

use crate::Database;
use chrono::Utc;
use norylum_foundation::{MediaId, NorylumError};
use rusqlite::params;
use serde::Serialize;

fn storage(error: impl std::fmt::Display) -> NorylumError {
    NorylumError::Internal(format!("SQLite Torrent V2 : {error}"))
}

#[derive(Debug, Clone, Serialize)]
pub struct TorrentShare {
    pub media_id: String,
    pub enabled: bool,
    pub status: String,
    pub magnet_uri: Option<String>,
    pub uploaded_bytes: u64,
    pub upload_bps: u64,
    pub peer_count: Option<u32>,
    pub last_error: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TorrentSettings {
    pub upload_kibps: u32,
}

impl Database {
    pub async fn torrent_settings(&self) -> Result<TorrentSettings, NorylumError> {
        self.with_connection(|conn| {
            let limit: u32 = conn
                .query_row(
                    "SELECT upload_kibps FROM torrent_settings WHERE id=1",
                    [],
                    |row| row.get(0),
                )
                .map_err(storage)?;

            Ok(TorrentSettings {
                upload_kibps: limit,
            })
        })
        .await
    }

    pub async fn set_torrent_limit(&self, limit: u32) -> Result<TorrentSettings, NorylumError> {
        if !(8..=512).contains(&limit) {
            return Err(NorylumError::Validation(
                "Limite requise : 8 a 512 Kio/s".into(),
            ));
        }

        self.with_connection(|conn| {
            conn.execute(
                "UPDATE torrent_settings
                 SET upload_kibps=?1, updated_at=?2
                 WHERE id=1",
                params![limit, Utc::now().to_rfc3339()],
            )
            .map_err(storage)?;

            Ok(TorrentSettings {
                upload_kibps: limit,
            })
        })
        .await
    }

    pub async fn torrent_shares(&self) -> Result<Vec<TorrentShare>, NorylumError> {
        self.with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT media_id,enabled,status,magnet_uri,
                            uploaded_bytes,upload_bps,peer_count,
                            last_error,updated_at
                     FROM torrent_shares ORDER BY updated_at DESC",
                )
                .map_err(storage)?;

            let rows = stmt
                .query_map([], |row| {
                    Ok(TorrentShare {
                        media_id: row.get(0)?,
                        enabled: row.get::<_, i64>(1)? != 0,
                        status: row.get(2)?,
                        magnet_uri: row.get(3)?,
                        uploaded_bytes: row.get::<_, u64>(4)?,
                        upload_bps: row.get::<_, u64>(5)?,
                        peer_count: row.get(6)?,
                        last_error: row.get(7)?,
                        updated_at: row.get(8)?,
                    })
                })
                .map_err(storage)?;

            rows.collect::<Result<Vec<_>, _>>().map_err(storage)
        })
        .await
    }

    pub async fn set_torrent_enabled(
        &self,
        media_id: &MediaId,
        enabled: bool,
    ) -> Result<(), NorylumError> {
        let id = media_id.as_str().to_owned();
        let now = Utc::now().to_rfc3339();

        self.with_connection(|conn| {
            if enabled {
                let changed = conn
                    .execute(
                        "INSERT INTO torrent_shares
                         (media_id,enabled,status,updated_at)
                         SELECT id,1,'queued',?2
                         FROM media_items WHERE id=?1
                         ON CONFLICT(media_id) DO UPDATE SET
                           enabled=1,
                           status='queued',
                           last_error=NULL,
                           updated_at=excluded.updated_at",
                        params![id, now],
                    )
                    .map_err(storage)?;

                if changed == 0 {
                    return Err(NorylumError::NotFound("Media non indexe".into()));
                }
            } else {
                let changed = conn
                    .execute(
                        "UPDATE torrent_shares
                         SET enabled=0,status='stopping',
                             upload_bps=0,updated_at=?2
                         WHERE media_id=?1",
                        params![id, now],
                    )
                    .map_err(storage)?;

                if changed == 0 {
                    return Err(NorylumError::NotFound("Partage torrent inconnu".into()));
                }
            }

            Ok(())
        })
        .await
    }

    pub async fn set_torrent_runtime_state(
        &self,
        media_id: &MediaId,
        enabled: bool,
        status: &str,
        magnet: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), NorylumError> {
        let now = Utc::now().to_rfc3339();

        self.with_connection(|conn| {
            conn.execute(
                "UPDATE torrent_shares SET
                   status=?3,
                   magnet_uri=COALESCE(?4,magnet_uri),
                   last_error=?5,
                   upload_bps=CASE WHEN ?2=0 THEN 0 ELSE upload_bps END,
                   updated_at=?6
                 WHERE media_id=?1 AND enabled=?2",
                params![
                    media_id.as_str(),
                    i32::from(enabled),
                    status,
                    magnet,
                    error,
                    now,
                ],
            )
            .map_err(storage)?;
            Ok(())
        })
        .await
    }

    pub async fn record_torrent_metrics(
        &self,
        media_id: &MediaId,
        uploaded_delta: u64,
        upload_bps: u64,
        peer_count: Option<u32>,
    ) -> Result<(), NorylumError> {
        let delta = i64::try_from(uploaded_delta).map_err(storage)?;
        let rate = i64::try_from(upload_bps).map_err(storage)?;

        self.with_connection(|conn| {
            conn.execute(
                "UPDATE torrent_shares SET
                   uploaded_bytes=uploaded_bytes+?2,
                   upload_bps=?3,
                   peer_count=?4,
                   status='seeding',
                   updated_at=?5
                 WHERE media_id=?1 AND enabled=1",
                params![
                    media_id.as_str(),
                    delta,
                    rate,
                    peer_count,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(storage)?;

            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn torrent_defaults_to_disabled() {
        let db = Database::in_memory().unwrap();

        assert!(db.torrent_shares().await.unwrap().is_empty());
        assert_eq!(db.torrent_settings().await.unwrap().upload_kibps, 64);

        assert!(db.set_torrent_limit(0).await.is_err());
        assert!(db.set_torrent_limit(513).await.is_err());
        assert_eq!(db.set_torrent_limit(128).await.unwrap().upload_kibps, 128);
    }

    #[tokio::test]
    async fn unknown_media_cannot_be_enabled() {
        let db = Database::in_memory().unwrap();

        assert!(matches!(
            db.set_torrent_enabled(&MediaId::new("med_1234567890abcdef"), true)
                .await,
            Err(NorylumError::NotFound(_))
        ));
    }
}

#[cfg(test)]
mod persistence_restart_tests {
    use super::*;
    use uuid::Uuid;

    #[tokio::test]
    async fn global_limit_survives_database_reopen() {
        let root = std::env::temp_dir().join(format!("norylum-torrent-db-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();

        let path = root.join("norylum.sqlite3");

        {
            let db = Database::open(&path).unwrap();
            db.set_torrent_limit(96).await.unwrap();
            assert!(db.torrent_shares().await.unwrap().is_empty());
        }

        {
            let db = Database::open(&path).unwrap();
            assert_eq!(db.torrent_settings().await.unwrap().upload_kibps, 96);
        }

        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod multi_share_tests {
    use super::*;
    use norylum_library::MediaItem;
    use std::path::PathBuf;

    fn movie(name: &str, id: &str) -> MediaItem {
        MediaItem {
            id: MediaId::new(id),
            title: name.into(),
            file_path: PathBuf::from(format!("/tmp/{name}.mkv")),
            relative_path: format!("{name} (2026).mkv"),
            file_size: 100,
            duration_seconds: 3.0,
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
    async fn shares_are_independently_enabled() {
        let db = Database::in_memory().unwrap();

        let a = movie("Alpha", "med_1111111111111111");
        let b = movie("Beta", "med_2222222222222222");

        db.sync_media_items(&[a.clone(), b.clone()]).await.unwrap();

        db.set_torrent_enabled(&a.id, true).await.unwrap();
        db.set_torrent_enabled(&b.id, true).await.unwrap();

        let shares = db.torrent_shares().await.unwrap();

        assert_eq!(shares.len(), 2);
        assert!(shares.iter().all(|s| s.enabled));

        db.set_torrent_enabled(&a.id, false).await.unwrap();

        let shares = db.torrent_shares().await.unwrap();

        let alpha = shares.iter().find(|s| s.media_id == a.id.as_str()).unwrap();

        let beta = shares.iter().find(|s| s.media_id == b.id.as_str()).unwrap();

        assert!(!alpha.enabled);
        assert_eq!(alpha.status, "stopping");

        assert!(beta.enabled);
        assert_eq!(beta.status, "queued");

        db.set_torrent_runtime_state(&a.id, false, "disabled", None, None)
            .await
            .unwrap();

        db.record_torrent_metrics(&b.id, 4096, 1024, None)
            .await
            .unwrap();

        let shares = db.torrent_shares().await.unwrap();

        let alpha = shares.iter().find(|s| s.media_id == a.id.as_str()).unwrap();

        let beta = shares.iter().find(|s| s.media_id == b.id.as_str()).unwrap();

        assert_eq!(alpha.status, "disabled");
        assert_eq!(alpha.uploaded_bytes, 0);

        assert_eq!(beta.status, "seeding");
        assert_eq!(beta.uploaded_bytes, 4096);
    }
}
