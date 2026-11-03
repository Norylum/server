//! Synchronisation atomique des progressions entre appareils.

use crate::{Database, PlaybackProgressRecord};
use chrono::Utc;
use norylum_foundation::{AccountId, MediaId, NorylumError, ProfileId};
use rusqlite::{params, OptionalExtension, TransactionBehavior};

fn storage(e: impl std::fmt::Display) -> NorylumError {
    NorylumError::Internal(format!("Erreur synchronisation SQLite : {e}"))
}

pub(crate) fn validate_progress(position: f64, duration: f64) -> Result<(), NorylumError> {
    if !position.is_finite()
        || !duration.is_finite()
        || duration <= 0.0
        || position < 0.0
        || position > duration
    {
        return Err(NorylumError::Validation(
            "Position et duree invalides".into(),
        ));
    }

    Ok(())
}

impl Database {
    /// Ecriture conditionnelle d'une progression.
    ///
    /// expected_revision = None :
    ///   creation seulement, si la progression est absente.
    ///
    /// expected_revision = Some(n) :
    ///   modification seulement si la revision actuelle est n.
    ///
    /// Le verrou SQLite protege la verification et l'ecriture.
    pub async fn sync_playback_progress(
        &self,
        account_id: &AccountId,
        profile_id: &ProfileId,
        media_id: &MediaId,
        position_seconds: f64,
        duration_seconds: f64,
        expected_revision: Option<u64>,
    ) -> Result<PlaybackProgressRecord, NorylumError> {
        validate_progress(position_seconds, duration_seconds)?;

        let expected = expected_revision
            .map(|v| {
                i64::try_from(v).map_err(|_| NorylumError::Validation("Revision invalide".into()))
            })
            .transpose()?;

        let completed = position_seconds / duration_seconds >= 0.90;

        let now = Utc::now();
        let stamp = now.to_rfc3339();

        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;

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

            let next = match current {
                Some(revision) => revision
                    .checked_add(1)
                    .ok_or_else(|| NorylumError::Internal("Revision maximale atteinte".into()))?,
                None => 1,
            };

            tx.execute(
                "INSERT INTO playback_progress (
                    account_id, profile_id, media_id,
                    position_seconds, duration_seconds,
                    is_completed, updated_at, revision
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
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
                    position_seconds,
                    duration_seconds,
                    i64::from(completed),
                    stamp,
                    next,
                ],
            )
            .map_err(storage)?;

            tx.commit().map_err(storage)?;

            Ok(PlaybackProgressRecord {
                account_id: account_id.clone(),
                profile_id: profile_id.clone(),
                media_id: media_id.clone(),
                position_seconds,
                duration_seconds,
                is_completed: completed,
                updated_at: now,
                revision: u64::try_from(next).map_err(storage)?,
            })
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_revision_conflict_and_legacy_write() {
        let db = Database::in_memory().unwrap();

        let account = AccountId::new("acc_test");
        let profile = ProfileId::new("prof_test");
        let media = MediaId::new("med_test");

        let first = db
            .sync_playback_progress(&account, &profile, &media, 20.0, 100.0, None)
            .await
            .unwrap();

        assert_eq!(first.revision, 1);

        assert!(matches!(
            db.sync_playback_progress(&account, &profile, &media, 30.0, 100.0, None,)
                .await,
            Err(NorylumError::Conflict(_))
        ));

        let second = db
            .sync_playback_progress(&account, &profile, &media, 60.0, 100.0, Some(1))
            .await
            .unwrap();

        assert_eq!(second.revision, 2);

        assert!(matches!(
            db.sync_playback_progress(&account, &profile, &media, 10.0, 100.0, Some(1),)
                .await,
            Err(NorylumError::Conflict(_))
        ));

        // L'ancienne API doit aussi incrementer la revision.
        let legacy = db
            .save_playback_progress(&account, &profile, &media, 95.0, 100.0)
            .await
            .unwrap();

        assert_eq!(legacy.revision, 3);
        assert!(legacy.is_completed);

        let stored = db
            .get_playback_progress(&account, &profile, &media)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(stored.position_seconds, 95.0);
        assert_eq!(stored.revision, 3);

        assert!(matches!(
            db.sync_playback_progress(&account, &profile, &media, f64::NAN, 100.0, Some(3),)
                .await,
            Err(NorylumError::Validation(_))
        ));

        assert!(matches!(
            db.sync_playback_progress(&account, &profile, &media, 101.0, 100.0, Some(3),)
                .await,
            Err(NorylumError::Validation(_))
        ));
    }
}
