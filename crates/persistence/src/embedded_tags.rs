use crate::Database;
use norylum_foundation::{MediaId, NorylumError};
use rusqlite::{params, OptionalExtension};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedMediaTags {
    pub tmdb_ref: String,
    pub provenance: String,
    pub season: Option<u32>,
    pub episode: Option<u32>,
}

impl Database {
    /// Resolves movie tags or series tags for a known episode.
    pub async fn embedded_tags_for_media(
        &self,
        media_id: &MediaId,
    ) -> Result<Option<EmbeddedMediaTags>, NorylumError> {
        if let Some(movie) = self.get_movie_metadata(media_id).await? {
            return Ok(Some(EmbeddedMediaTags {
                tmdb_ref: movie.tmdb_ref,
                provenance: "Norylum Server".into(),
                season: None,
                episode: None,
            }));
        }

        let episode: Option<(i64, i64, i64)> = self
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT m.tmdb_id,
                            s.season_number,
                            e.episode_number
                     FROM episodes e
                     JOIN seasons s
                       ON s.id = e.season_id
                     JOIN show_metadata m
                       ON m.show_id = e.show_id
                     WHERE e.media_id = ?1",
                    params![media_id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(|e| NorylumError::Internal(e.to_string()))
            })
            .await?;

        let Some((id, season, episode)) = episode else {
            return Ok(None);
        };

        let id = u64::try_from(id).map_err(|e| NorylumError::Internal(e.to_string()))?;

        if id == 0 {
            return Err(NorylumError::Validation("TMDB ID invalide".into()));
        }

        let season = u32::try_from(season).map_err(|e| NorylumError::Internal(e.to_string()))?;

        let episode = u32::try_from(episode).map_err(|e| NorylumError::Internal(e.to_string()))?;

        Ok(Some(EmbeddedMediaTags {
            tmdb_ref: format!("tv/{id}"),
            provenance: "Norylum Server".into(),
            season: Some(season),
            episode: Some(episode),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use norylum_library::MediaItem;
    use norylum_metadata::{TmdbItem, TmdbKind};
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_movie_tags_have_generic_provenance() {
        let db = Database::in_memory().unwrap();
        let media = MediaItem {
            id: MediaId::new("med_embed_test"),
            title: "Film Test".into(),
            file_path: PathBuf::from("/tmp/Film Test (2000).mkv"),
            relative_path: "Film Test (2000).mkv".into(),
            file_size: 100,
            duration_seconds: 10.0,
            mime_type: "video/x-matroska".into(),
            video_codec: None,
            audio_codec: None,
            width: None,
            height: None,
            is_child_safe: true,
            age_rating: Some(0),
        };

        db.sync_media_items(std::slice::from_ref(&media))
            .await
            .unwrap();

        db.set_movie_metadata(
            &media.id,
            &TmdbItem {
                tmdb_id: 603,
                tmdb_ref: "movie/603".into(),
                kind: TmdbKind::Movie,
                title: "Film".into(),
                overview: String::new(),
                poster_path: None,
                poster_url: None,
                release_date: None,
            },
        )
        .await
        .unwrap();

        let tags = db
            .embedded_tags_for_media(&media.id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(tags.tmdb_ref, "movie/603");
        assert_eq!(tags.provenance, "Norylum Server");
        assert!(tags.season.is_none());
        assert!(tags.episode.is_none());

        let missing = db
            .embedded_tags_for_media(&MediaId::new("med_missing"))
            .await
            .unwrap();

        assert!(missing.is_none());
    }
}

#[cfg(test)]
mod episode_tests {
    use super::*;
    use norylum_library::MediaItem;
    use norylum_metadata::{TmdbItem, TmdbKind};
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_episode_uses_series_tmdb_id() {
        let db = Database::in_memory().unwrap();

        let episode = MediaItem {
            id: MediaId::new("med_episode_tags"),
            title: "Test Show S03E05".into(),
            file_path: PathBuf::from("/tmp/Test Show (2020)/Season 03/Test Show S03E05.mkv"),
            relative_path: "Test Show (2020)/Season 03/Test Show S03E05.mkv".into(),
            file_size: 100,
            duration_seconds: 1200.0,
            mime_type: "video/x-matroska".into(),
            video_codec: None,
            audio_codec: None,
            width: None,
            height: None,
            is_child_safe: true,
            age_rating: Some(0),
        };

        db.sync_media_items(std::slice::from_ref(&episode))
            .await
            .unwrap();

        let shows = db.list_all_shows().await.unwrap();
        assert_eq!(shows.len(), 1);

        db.set_show_metadata(
            &shows[0].id,
            &TmdbItem {
                tmdb_id: 1396,
                tmdb_ref: "tv/1396".into(),
                kind: TmdbKind::Tv,
                title: "Test Show".into(),
                overview: String::new(),
                poster_path: None,
                poster_url: None,
                release_date: None,
            },
        )
        .await
        .unwrap();

        let tags = db
            .embedded_tags_for_media(&episode.id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(tags.tmdb_ref, "tv/1396");
        assert_eq!(tags.season, Some(3));
        assert_eq!(tags.episode, Some(5));
        assert_eq!(tags.provenance, "Norylum Server");
    }
}
