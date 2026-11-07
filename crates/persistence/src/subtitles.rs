use crate::Database;
use norylum_foundation::NorylumError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleTrack {
    pub id: String,
    pub media_id: String,
    pub lang: String,
    pub format: String,
    pub relative_path: String,
}

impl Database {
    pub async fn upsert_subtitle_track(
        &self,
        id: &str,
        media_id: &str,
        lang: &str,
        format: &str,
        relative_path: &str,
    ) -> Result<(), NorylumError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO subtitle_tracks (id, media_id, lang, format, relative_path)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(relative_path) DO UPDATE SET
               media_id=excluded.media_id,
               lang=excluded.lang,
               format=excluded.format",
            rusqlite::params![id, media_id, lang, format, relative_path],
        )
        .map_err(|e| NorylumError::Internal(e.to_string()))?;
        Ok(())
    }

    pub async fn subtitle_tracks_for_media(
        &self,
        media_id: &str,
    ) -> Result<Vec<SubtitleTrack>, NorylumError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, media_id, lang, format, relative_path
                 FROM subtitle_tracks
                 WHERE media_id = ?1
                 ORDER BY lang",
            )
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        let rows = stmt
            .query_map(rusqlite::params![media_id], |row| {
                Ok(SubtitleTrack {
                    id: row.get(0)?,
                    media_id: row.get(1)?,
                    lang: row.get(2)?,
                    format: row.get(3)?,
                    relative_path: row.get(4)?,
                })
            })
            .map_err(|e| NorylumError::Internal(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        Ok(rows)
    }
}
