use crate::Database;
use chrono::{Duration, SecondsFormat, Utc};
use norylum_foundation::NorylumError;
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};

const LEASE_SECONDS: i64 = 90;
const MAX_ATTEMPTS: i64 = 3;

#[derive(Debug, Clone)]
pub struct AcquisitionLease {
    pub request_id: String,
    pub lease_token: String,
    pub attempt: i64,
}

fn storage(error: impl std::fmt::Display) -> NorylumError {
    NorylumError::Internal(format!("Acquisition worker SQLite : {error}"))
}

fn timestamp(value: chrono::DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn recover_expired(tx: &Transaction<'_>, now: &str) -> Result<usize, NorylumError> {
    let expired: Vec<(String, i64)> = {
        let mut statement = tx
            .prepare(
                "SELECT j.request_id, j.attempt_count
                 FROM acquisition_worker_leases j
                 JOIN acquisition_requests r ON r.id=j.request_id
                 WHERE r.status='searching'
                   AND j.lease_token IS NOT NULL
                   AND j.expires_at <= ?1",
            )
            .map_err(storage)?;

        let rows = statement
            .query_map(params![now], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(storage)?;

        rows.collect::<Result<Vec<_>, _>>().map_err(storage)?
    };

    for (id, attempts) in &expired {
        let next = if *attempts >= MAX_ATTEMPTS {
            "failed"
        } else {
            "approved"
        };

        tx.execute(
            "UPDATE acquisition_requests
             SET status=?2, updated_at=?3
             WHERE id=?1 AND status='searching'",
            params![id, next, now],
        )
        .map_err(storage)?;

        tx.execute(
            "UPDATE acquisition_worker_leases
             SET lease_token=NULL, expires_at=NULL, updated_at=?2
             WHERE request_id=?1",
            params![id, now],
        )
        .map_err(storage)?;
    }

    Ok(expired.len())
}

impl Database {
    /// Atomically reserves the oldest approved request.
    /// No network access or file operation takes place.
    pub async fn claim_next_approved_acquisition(
        &self,
    ) -> Result<Option<AcquisitionLease>, NorylumError> {
        let moment = Utc::now();
        let now = timestamp(moment);
        let until = timestamp(moment + Duration::seconds(LEASE_SECONDS));

        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;

            recover_expired(&tx, &now)?;

            let id: Option<String> = tx
                .query_row(
                    "SELECT id FROM acquisition_requests
                     WHERE status='approved'
                     ORDER BY created_at ASC, id ASC
                     LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;

            let Some(id) = id else {
                tx.commit().map_err(storage)?;
                return Ok(None);
            };

            let token = uuid::Uuid::new_v4().to_string();

            tx.execute(
                "INSERT INTO acquisition_worker_leases
                 (request_id,lease_token,expires_at,attempt_count,updated_at)
                 VALUES (?1,?2,?3,1,?4)
                 ON CONFLICT(request_id) DO UPDATE SET
                   lease_token=excluded.lease_token,
                   expires_at=excluded.expires_at,
                   attempt_count=acquisition_worker_leases.attempt_count+1,
                   updated_at=excluded.updated_at",
                params![&id, &token, &until, &now],
            )
            .map_err(storage)?;

            let changed = tx
                .execute(
                    "UPDATE acquisition_requests
                     SET status='searching', updated_at=?2
                     WHERE id=?1 AND status='approved'",
                    params![&id, &now],
                )
                .map_err(storage)?;

            if changed != 1 {
                return Err(NorylumError::Conflict(
                    "Reservation concurrente refusee".into(),
                ));
            }

            let attempt: i64 = tx
                .query_row(
                    "SELECT attempt_count
                     FROM acquisition_worker_leases
                     WHERE request_id=?1",
                    params![&id],
                    |row| row.get(0),
                )
                .map_err(storage)?;

            tx.commit().map_err(storage)?;

            Ok(Some(AcquisitionLease {
                request_id: id,
                lease_token: token,
                attempt,
            }))
        })
        .await
    }

    /// Extends only a live lease with the correct token.
    pub async fn renew_acquisition_claim(
        &self,
        lease: &AcquisitionLease,
    ) -> Result<(), NorylumError> {
        let moment = Utc::now();
        let now = timestamp(moment);
        let until = timestamp(moment + Duration::seconds(LEASE_SECONDS));
        let id = lease.request_id.clone();
        let token = lease.lease_token.clone();

        self.with_connection(|conn| {
            let changed = conn
                .execute(
                    "UPDATE acquisition_worker_leases
                     SET expires_at=?3, updated_at=?4
                     WHERE request_id=?1 AND lease_token=?2
                       AND expires_at > ?4
                       AND EXISTS (
                         SELECT 1 FROM acquisition_requests
                         WHERE id=?1 AND status='searching'
                       )",
                    params![id, token, until, now],
                )
                .map_err(storage)?;

            if changed != 1 {
                return Err(NorylumError::Conflict(
                    "Bail d'acquisition expire ou invalide".into(),
                ));
            }
            Ok(())
        })
        .await
    }

    /// Deliberately cannot mark a media as available.
    /// Availability requires a separately validated ingestion.
    pub async fn finish_acquisition_claim(
        &self,
        lease: &AcquisitionLease,
        terminal_status: &str,
    ) -> Result<(), NorylumError> {
        if !matches!(terminal_status, "failed" | "unavailable") {
            return Err(NorylumError::Validation(
                "Etat terminal du worker non autorise".into(),
            ));
        }

        let now = timestamp(Utc::now());
        let id = lease.request_id.clone();
        let token = lease.lease_token.clone();
        let target = terminal_status.to_owned();

        self.with_connection(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;

            let changed = tx
                .execute(
                    "UPDATE acquisition_requests
                     SET status=?3, updated_at=?4
                     WHERE id=?1 AND status='searching'
                       AND EXISTS (
                         SELECT 1 FROM acquisition_worker_leases
                         WHERE request_id=?1
                           AND lease_token=?2
                           AND expires_at > ?4
                       )",
                    params![&id, &token, &target, &now],
                )
                .map_err(storage)?;

            if changed != 1 {
                return Err(NorylumError::Conflict(
                    "Bail d'acquisition perdu ou expire".into(),
                ));
            }

            tx.execute(
                "UPDATE acquisition_worker_leases
                 SET lease_token=NULL, expires_at=NULL, updated_at=?2
                 WHERE request_id=?1",
                params![&id, &now],
            )
            .map_err(storage)?;

            tx.commit().map_err(storage)?;
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use norylum_foundation::{AccountId, ProfileId};

    #[tokio::test]
    async fn claims_are_exclusive_and_expired_tokens_are_rejected() {
        let root =
            std::env::temp_dir().join(format!("norylum-worker-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();

        let path = root.join("worker.sqlite3");
        let db1 = Database::open(&path).unwrap();
        let db2 = Database::open(&path).unwrap();

        let account = AccountId::new("acc_worker");
        let profile = ProfileId::new("prof_worker");

        let request = db1
            .request_acquisition(
                &account, &profile, "movie", 987650104, None, None, "1080p", "fr",
            )
            .await
            .unwrap();

        db1.approve_acquisition_request(&account, &request.id)
            .await
            .unwrap();

        let (a, b) = tokio::join!(
            db1.claim_next_approved_acquisition(),
            db2.claim_next_approved_acquisition()
        );

        let a = a.unwrap();
        let b = b.unwrap();

        assert_ne!(a.is_some(), b.is_some());

        let first = a.or(b).unwrap();
        assert_eq!(first.request_id, request.id);
        assert_eq!(first.attempt, 1);

        // Simulate expiration of the worker's lease.
        db1.with_connection(|conn| {
            conn.execute(
                "UPDATE acquisition_worker_leases
                 SET expires_at='2000-01-01T00:00:00.000Z'
                 WHERE request_id=?1",
                params![&first.request_id],
            )
            .map_err(storage)?;
            Ok(())
        })
        .await
        .unwrap();

        let second = db2
            .claim_next_approved_acquisition()
            .await
            .unwrap()
            .unwrap();

        assert_eq!(second.request_id, request.id);
        assert_eq!(second.attempt, 2);
        assert_ne!(second.lease_token, first.lease_token);

        assert!(db1.renew_acquisition_claim(&first).await.is_err());
        assert!(db1
            .finish_acquisition_claim(&first, "failed")
            .await
            .is_err());

        assert!(db2
            .finish_acquisition_claim(&second, "available")
            .await
            .is_err());

        db2.renew_acquisition_claim(&second).await.unwrap();

        db2.finish_acquisition_claim(&second, "unavailable")
            .await
            .unwrap();

        let completed = db1
            .get_acquisition_request(&account, &request.id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(completed.status, "unavailable");

        drop(db1);
        drop(db2);

        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod attempt_limit_tests {
    use super::*;
    use norylum_foundation::{AccountId, ProfileId};

    #[tokio::test]
    async fn worker_stops_after_three_expired_claims() {
        let db = Database::in_memory().unwrap();
        let account = AccountId::new("acc_attempt_limit");
        let profile = ProfileId::new("prof_attempt_limit");

        let request = db
            .request_acquisition(
                &account, &profile, "movie", 987650105, None, None, "1080p", "fr",
            )
            .await
            .unwrap();

        db.approve_acquisition_request(&account, &request.id)
            .await
            .unwrap();

        for expected_attempt in 1..=3 {
            let lease = db
                .claim_next_approved_acquisition()
                .await
                .unwrap()
                .expect("Reservation attendue");

            assert_eq!(lease.request_id, request.id);
            assert_eq!(lease.attempt, expected_attempt);

            db.with_connection(|conn| {
                conn.execute(
                    "UPDATE acquisition_worker_leases
                     SET expires_at='2000-01-01T00:00:00.000Z'
                     WHERE request_id=?1",
                    params![&lease.request_id],
                )
                .map_err(storage)?;

                Ok(())
            })
            .await
            .unwrap();
        }

        // Recovery of the third expired lease must fail the request.
        assert!(db
            .claim_next_approved_acquisition()
            .await
            .unwrap()
            .is_none());

        let failed = db
            .get_acquisition_request(&account, &request.id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(failed.status, "failed");

        // No fourth attempt may be issued.
        assert!(db
            .claim_next_approved_acquisition()
            .await
            .unwrap()
            .is_none());
    }
}
