//! Rejeu idempotent des ecritures de progression.

use crate::{playback_sync::validate_progress, Database, PlaybackProgressRecord};
use chrono::{DateTime, Utc};
use norylum_foundation::{AccountId, MediaId, NorylumError, ProfileId};
use rusqlite::{params, OptionalExtension, TransactionBehavior};

const RETENTION_SECONDS: i64 = 7 * 24 * 60 * 60;

fn storage(error: impl std::fmt::Display) -> NorylumError {
    NorylumError::Internal(format!("Erreur Playback Sync V2 : {error}"))
}

#[derive(Debug, Clone, Copy)]
pub struct SyncWriteOperation<'a> {
    pub operation_id: &'a str,
    pub position_seconds: f64,
    pub duration_seconds: f64,
    pub expected_revision: Option<u64>,
}

impl Database {
    /// Ecriture idempotente.
    ///
    /// Une operation deja appliquee est retournee telle quelle,
    /// meme si une autre ecriture a eu lieu depuis.
    /// Elle n'ecrase jamais l'etat actuel.
    pub async fn sync_playback_progress_idempotent(
        &self,
        account_id: &AccountId,
        profile_id: &ProfileId,
        media_id: &MediaId,
        write: SyncWriteOperation<'_>,
    ) -> Result<PlaybackProgressRecord, NorylumError> {
        validate_progress(write.position_seconds, write.duration_seconds)?;

        if !(8..=64).contains(&write.operation_id.len())
            || !write
                .operation_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(NorylumError::Validation(
                "operation_id invalide (8-64 caracteres ASCII)".into(),
            ));
        }

        let expected = write
            .expected_revision
            .map(|value| {
                i64::try_from(value)
                    .map_err(|_| NorylumError::Validation("Revision invalide".into()))
            })
            .transpose()?;

        let now = Utc::now();
        let now_seconds = now.timestamp();
        let updated_at = now.to_rfc3339();

        let completed = write.position_seconds / write.duration_seconds >= 0.90;

        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;

            // Nettoyage des anciens identifiants d'operation.
            // Les progressions elles-memes ne sont pas effacees.
            tx.execute(
                "DELETE FROM playback_sync_operations
                 WHERE created_at < ?1",
                params![now_seconds.saturating_sub(RETENTION_SECONDS)],
            )
            .map_err(storage)?;

            let previous: Option<(Option<i64>, f64, f64, i64, i64, String)> = tx
                .query_row(
                    "SELECT expected_revision,
                        position_seconds,
                        duration_seconds,
                        is_completed,
                        revision,
                        updated_at
                 FROM playback_sync_operations
                 WHERE account_id = ?1
                   AND profile_id = ?2
                   AND media_id = ?3
                   AND operation_id = ?4",
                    params![
                        account_id.as_str(),
                        profile_id.as_str(),
                        media_id.as_str(),
                        write.operation_id,
                    ],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    },
                )
                .optional()
                .map_err(storage)?;

            if let Some((
                prior_expected,
                prior_position,
                prior_duration,
                prior_completed,
                prior_revision,
                prior_timestamp,
            )) = previous
            {
                if prior_expected != expected
                    || prior_position != write.position_seconds
                    || prior_duration != write.duration_seconds
                {
                    return Err(NorylumError::Validation(
                        "operation_id deja utilise avec une autre ecriture".into(),
                    ));
                }

                let parsed = DateTime::parse_from_rfc3339(&prior_timestamp)
                    .map_err(storage)?
                    .with_timezone(&Utc);

                return Ok(PlaybackProgressRecord {
                    account_id: account_id.clone(),
                    profile_id: profile_id.clone(),
                    media_id: media_id.clone(),
                    position_seconds: prior_position,
                    duration_seconds: prior_duration,
                    is_completed: prior_completed != 0,
                    updated_at: parsed,
                    revision: u64::try_from(prior_revision).map_err(storage)?,
                });
            }

            let current: Option<i64> = tx
                .query_row(
                    "SELECT revision
                     FROM playback_progress
                     WHERE account_id = ?1
                       AND profile_id = ?2
                       AND media_id = ?3",
                    params![account_id.as_str(), profile_id.as_str(), media_id.as_str(),],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;

            if current != expected {
                return Err(NorylumError::Conflict(
                    "Progression modifiee sur un autre appareil".into(),
                ));
            }

            let revision = match current {
                Some(value) => value
                    .checked_add(1)
                    .ok_or_else(|| NorylumError::Internal("Revision maximale atteinte".into()))?,
                None => 1,
            };

            tx.execute(
                "INSERT INTO playback_progress (
                    account_id, profile_id, media_id,
                    position_seconds, duration_seconds,
                    is_completed, updated_at, revision
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8
                 )
                 ON CONFLICT (
                    account_id, profile_id, media_id
                 ) DO UPDATE SET
                    position_seconds = excluded.position_seconds,
                    duration_seconds = excluded.duration_seconds,
                    is_completed = excluded.is_completed,
                    updated_at = excluded.updated_at,
                    revision = excluded.revision",
                params![
                    account_id.as_str(),
                    profile_id.as_str(),
                    media_id.as_str(),
                    write.position_seconds,
                    write.duration_seconds,
                    i64::from(completed),
                    updated_at,
                    revision,
                ],
            )
            .map_err(storage)?;

            tx.execute(
                "INSERT INTO playback_sync_operations (
                    account_id, profile_id, media_id,
                    operation_id, expected_revision,
                    position_seconds, duration_seconds,
                    is_completed, revision, updated_at,
                    created_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6,
                    ?7, ?8, ?9, ?10, ?11
                 )",
                params![
                    account_id.as_str(),
                    profile_id.as_str(),
                    media_id.as_str(),
                    write.operation_id,
                    expected,
                    write.position_seconds,
                    write.duration_seconds,
                    i64::from(completed),
                    revision,
                    updated_at,
                    now_seconds,
                ],
            )
            .map_err(storage)?;

            tx.commit().map_err(storage)?;

            Ok(PlaybackProgressRecord {
                account_id: account_id.clone(),
                profile_id: profile_id.clone(),
                media_id: media_id.clone(),
                position_seconds: write.position_seconds,
                duration_seconds: write.duration_seconds,
                is_completed: completed,
                updated_at: now,
                revision: u64::try_from(revision).map_err(storage)?,
            })
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "operation-0001";

    #[tokio::test]
    async fn test_lost_response_retry_and_conflict() {
        let db = Database::in_memory().unwrap();

        let account = AccountId::new("acc_sync");
        let profile = ProfileId::new("prof_sync");
        let media = MediaId::new("med_sync");

        let initial = SyncWriteOperation {
            operation_id: ID,
            position_seconds: 20.0,
            duration_seconds: 100.0,
            expected_revision: None,
        };

        let first = db
            .sync_playback_progress_idempotent(&account, &profile, &media, initial)
            .await
            .unwrap();

        assert_eq!(first.revision, 1);

        // Rejouer apres une coupure ne recree rien.
        let retry = db
            .sync_playback_progress_idempotent(&account, &profile, &media, initial)
            .await
            .unwrap();

        assert_eq!(retry.revision, 1);
        assert_eq!(retry.updated_at, first.updated_at);

        let newer = db
            .sync_playback_progress_idempotent(
                &account,
                &profile,
                &media,
                SyncWriteOperation {
                    operation_id: "operation-0002",
                    position_seconds: 80.0,
                    duration_seconds: 100.0,
                    expected_revision: Some(1),
                },
            )
            .await
            .unwrap();

        assert_eq!(newer.revision, 2);

        // Ancien replay : accuse la premiere operation,
        // mais ne remet surtout pas le film a 20 secondes.
        let old_retry = db
            .sync_playback_progress_idempotent(&account, &profile, &media, initial)
            .await
            .unwrap();

        assert_eq!(old_retry.revision, 1);

        let current = db
            .get_playback_progress(&account, &profile, &media)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(current.position_seconds, 80.0);
        assert_eq!(current.revision, 2);

        assert!(matches!(
            db.sync_playback_progress_idempotent(
                &account,
                &profile,
                &media,
                SyncWriteOperation {
                    operation_id: ID,
                    position_seconds: 30.0,
                    ..initial
                },
            )
            .await,
            Err(NorylumError::Validation(_))
        ));

        assert!(matches!(
            db.sync_playback_progress_idempotent(
                &account,
                &profile,
                &media,
                SyncWriteOperation {
                    operation_id: "operation-0003",
                    position_seconds: 30.0,
                    duration_seconds: 100.0,
                    expected_revision: Some(1),
                },
            )
            .await,
            Err(NorylumError::Conflict(_))
        ));
    }
}
