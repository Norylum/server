//! Authenticated acquisition intent. No network or download occurs here.

use crate::Database;
use chrono::Utc;
use norylum_foundation::{AccountId, NorylumError, ProfileId};
use rusqlite::{params, OptionalExtension, Row, TransactionBehavior};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct AcquisitionRequest {
    pub id: String,
    pub kind: String,
    pub tmdb_id: u64,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub quality_profile: String,
    pub language: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

fn storage(error: impl std::fmt::Display) -> NorylumError {
    NorylumError::Internal(format!("Acquisition SQLite : {error}"))
}

fn map_request(row: &Row<'_>) -> rusqlite::Result<AcquisitionRequest> {
    Ok(AcquisitionRequest {
        id: row.get(0)?,
        kind: row.get(1)?,
        tmdb_id: row.get(2)?,
        season: row.get(3)?,
        episode: row.get(4)?,
        quality_profile: row.get(5)?,
        language: row.get(6)?,
        status: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

const COLUMNS: &str = "id,kind,tmdb_id,season,episode,
                      quality_profile,language,status,
                      created_at,updated_at";

impl Database {
    #[allow(clippy::too_many_arguments)]
    pub async fn request_acquisition(
        &self,
        account_id: &AccountId,
        profile_id: &ProfileId,
        kind: &str,
        tmdb_id: u64,
        season: Option<u32>,
        episode: Option<u32>,
        quality: &str,
        language: &str,
    ) -> Result<AcquisitionRequest, NorylumError> {
        let tmdb = i64::try_from(tmdb_id).map_err(storage)?;

        if tmdb <= 0
            || !matches!(kind, "movie" | "tv")
            || (kind == "movie" && (season.is_some() || episode.is_some()))
            || (kind == "tv" && episode.is_some() && season.is_none())
            || season.is_some_and(|n| n > 999)
            || episode.is_some_and(|n| n == 0 || n > 999)
            || !matches!(quality, "720p" | "1080p" | "2160p")
            || !matches!(language, "fr" | "en" | "original")
        {
            return Err(NorylumError::Validation(
                "Parametres d'acquisition invalides".into(),
            ));
        }

        let account = account_id.as_str().to_owned();
        let profile = profile_id.as_str().to_owned();
        let kind = kind.to_owned();
        let quality = quality.to_owned();
        let language = language.to_owned();

        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;

            // A movie already indexed with this TMDB reference
            // must not be requested a second time.
            if kind == "movie" {
                let present: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM media_metadata
                         WHERE tmdb_id=?1",
                        params![tmdb],
                        |row| row.get(0),
                    )
                    .map_err(storage)?;

                if present > 0 {
                    return Err(NorylumError::Conflict(
                        "Film deja present dans la bibliotheque".into(),
                    ));
                }
            }

            let id = format!("acq_{}", uuid::Uuid::new_v4().simple());
            let now = Utc::now().to_rfc3339();

            tx.execute(
                "INSERT INTO acquisition_requests
                 (id,kind,tmdb_id,season,episode,
                  quality_profile,language,status,
                  account_id,profile_id,created_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,
                         'awaiting_approval',?8,?9,?10,?10)
                 ON CONFLICT DO NOTHING",
                params![
                    id,
                    kind,
                    tmdb,
                    season.map(i64::from),
                    episode.map(i64::from),
                    quality,
                    language,
                    account,
                    profile,
                    now,
                ],
            )
            .map_err(storage)?;

            // Return the existing active request if deduplicated.
            let query = format!(
                "SELECT {COLUMNS} FROM acquisition_requests
                 WHERE account_id=?1 AND kind=?2 AND tmdb_id=?3
                   AND COALESCE(season,-1)=COALESCE(?4,-1)
                   AND COALESCE(episode,-1)=COALESCE(?5,-1)
                   AND status IN (
                     'awaiting_approval','approved','searching',
                     'downloading','processing'
                   )
                 LIMIT 1"
            );

            let request = tx
                .query_row(
                    &query,
                    params![
                        account,
                        kind,
                        tmdb,
                        season.map(i64::from),
                        episode.map(i64::from)
                    ],
                    map_request,
                )
                .map_err(storage)?;

            tx.commit().map_err(storage)?;
            Ok(request)
        })
        .await
    }

    pub async fn list_acquisition_requests(
        &self,
        account_id: &AccountId,
    ) -> Result<Vec<AcquisitionRequest>, NorylumError> {
        let account = account_id.as_str().to_owned();

        self.with_connection(|conn| {
            let query = format!(
                "SELECT {COLUMNS} FROM acquisition_requests
                 WHERE account_id=?1
                 ORDER BY created_at DESC LIMIT 100"
            );

            let mut stmt = conn.prepare(&query).map_err(storage)?;

            let rows = stmt
                .query_map(params![account], map_request)
                .map_err(storage)?;

            rows.collect::<Result<Vec<_>, _>>().map_err(storage)
        })
        .await
    }

    pub async fn get_acquisition_request(
        &self,
        account_id: &AccountId,
        id: &str,
    ) -> Result<Option<AcquisitionRequest>, NorylumError> {
        let account = account_id.as_str().to_owned();
        let id = id.to_owned();

        self.with_connection(|conn| {
            let query = format!(
                "SELECT {COLUMNS} FROM acquisition_requests
                 WHERE account_id=?1 AND id=?2"
            );

            conn.query_row(&query, params![account, id], map_request)
                .optional()
                .map_err(storage)
        })
        .await
    }
}
