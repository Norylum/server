//! Cycle de vie des appareils Norylum.
//! Renommage, maintenance et oubli de l'historique.

use crate::{storage, DeviceService};
use chrono::Utc;
use norylum_foundation::{DeviceId, NorylumError};
use rusqlite::{params, TransactionBehavior};
use serde::Serialize;

const DEVICE_RETENTION_SECONDS: i64 = 30 * 24 * 60 * 60;

#[derive(Debug, Clone, Serialize)]
pub struct MaintenanceReport {
    pub expired_sessions_removed: u64,
    pub inactive_devices_removed: u64,
}

impl DeviceService {
    /// Renomme un appareil appartenant au compte.
    /// Reserve au profil principal.
    pub async fn rename_device(
        &self,
        token: &str,
        device_id: &DeviceId,
        name: &str,
    ) -> Result<(), NorylumError> {
        let name = name.trim();

        if name.is_empty() || name.len() > 64 || name.chars().any(char::is_control) {
            return Err(NorylumError::Validation(
                "Nom d'appareil invalide (1-64 octets)".into(),
            ));
        }

        let now = Utc::now().timestamp();

        self.db
            .with_connection(|conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(storage)?;

                let (account, _, _) = Self::principal(&tx, token, now)?;

                let updated = tx
                    .execute(
                        "UPDATE devices SET name = ?3
                 WHERE id = ?1 AND account_id = ?2",
                        params![device_id.as_str(), account.as_str(), name,],
                    )
                    .map_err(storage)?;

                if updated == 0 {
                    return Err(NorylumError::NotFound("Appareil introuvable".into()));
                }

                tx.commit().map_err(storage)?;
                Ok(())
            })
            .await
    }

    /// Retire les sessions expirees du compte.
    /// Supprime ensuite les appareils sans session
    /// et inactifs depuis au moins 30 jours.
    ///
    /// Aucun autre compte n'est modifie.
    /// L'operation est explicite et transactionnelle.
    pub async fn maintenance(&self, token: &str) -> Result<MaintenanceReport, NorylumError> {
        let now = Utc::now().timestamp();
        let cutoff = now.saturating_sub(DEVICE_RETENTION_SECONDS);

        self.db
            .with_connection(|conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(storage)?;

                let (account, _, _) = Self::principal(&tx, token, now)?;

                let expired = tx
                    .execute(
                        "DELETE FROM auth_sessions
                 WHERE account_id = ?1
                   AND expires_at <= ?2",
                        params![account.as_str(), now],
                    )
                    .map_err(storage)?;

                let inactive = tx
                    .execute(
                        "DELETE FROM devices
                 WHERE account_id = ?1
                   AND last_seen_at < ?2
                   AND NOT EXISTS (
                       SELECT 1
                       FROM auth_sessions s
                       WHERE s.device_id = devices.id
                         AND s.account_id = devices.account_id
                   )",
                        params![account.as_str(), cutoff],
                    )
                    .map_err(storage)?;

                tx.commit().map_err(storage)?;

                Ok(MaintenanceReport {
                    expired_sessions_removed: expired as u64,
                    inactive_devices_removed: inactive as u64,
                })
            })
            .await
    }

    /// Efface un appareil de l'historique uniquement
    /// lorsqu'il n'a plus aucune session associee.
    pub async fn forget_device(
        &self,
        token: &str,
        device_id: &DeviceId,
    ) -> Result<(), NorylumError> {
        let now = Utc::now().timestamp();

        self.db
            .with_connection(|conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(storage)?;

                let (account, _, _) = Self::principal(&tx, token, now)?;

                let exists: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM devices
                 WHERE id = ?1 AND account_id = ?2",
                        params![device_id.as_str(), account.as_str(),],
                        |row| row.get(0),
                    )
                    .map_err(storage)?;

                if exists == 0 {
                    return Err(NorylumError::NotFound("Appareil introuvable".into()));
                }

                let sessions: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM auth_sessions
                 WHERE device_id = ?1 AND account_id = ?2",
                        params![device_id.as_str(), account.as_str(),],
                        |row| row.get(0),
                    )
                    .map_err(storage)?;

                if sessions != 0 {
                    return Err(NorylumError::Validation(
                        "Revoquer ou nettoyer les sessions avant d'oublier cet appareil".into(),
                    ));
                }

                tx.execute(
                    "DELETE FROM devices
                 WHERE id = ?1 AND account_id = ?2",
                    params![device_id.as_str(), account.as_str(),],
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
    use norylum_auth::AuthService;
    use norylum_foundation::{AccountId, ProfileId};
    use norylum_persistence::Database;

    const PASSWORD: &str = "devices-v2-test-password-2026";

    async fn add_other_account(db: &Database) {
        let account = AccountId::random();
        let profile = ProfileId::random();

        db.with_connection(|conn| {
            let encoded: String = conn
                .query_row(
                    "SELECT password_hash FROM accounts
                 WHERE username = 'admin'",
                    [],
                    |row| row.get(0),
                )
                .map_err(storage)?;

            conn.execute(
                "INSERT INTO accounts
                 (id, username, password_hash)
                 VALUES (?1, 'other', ?2)",
                params![account.as_str(), encoded],
            )
            .map_err(storage)?;

            conn.execute(
                "INSERT INTO profiles
                 (id, account_id, name, is_child,
                  age_rating_max)
                 VALUES (?1, ?2, 'Principal', 0, NULL)",
                params![profile.as_str(), account.as_str()],
            )
            .map_err(storage)?;

            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_rename_forget_and_account_isolation() {
        let db = Database::in_memory().unwrap();
        let auth = AuthService::new(db.clone());
        let devices = DeviceService::new(db.clone());

        auth.bootstrap_admin("admin", PASSWORD).await.unwrap();

        let mac = auth
            .login_with_device("admin", PASSWORD, Some("Mac"))
            .await
            .unwrap();

        let tv = auth
            .login_with_device("admin", PASSWORD, Some("TV"))
            .await
            .unwrap();

        add_other_account(&db).await;

        let foreign = auth
            .login_with_device("other", PASSWORD, Some("Autre compte"))
            .await
            .unwrap();

        assert!(matches!(
            devices
                .rename_device(&mac.access_token, &foreign.device_id, "Falsifie")
                .await,
            Err(NorylumError::NotFound(_))
        ));

        assert!(matches!(
            devices
                .forget_device(&mac.access_token, &foreign.device_id)
                .await,
            Err(NorylumError::NotFound(_))
        ));

        assert!(matches!(
            devices
                .rename_device(&mac.access_token, &tv.device_id, "  ")
                .await,
            Err(NorylumError::Validation(_))
        ));

        devices
            .rename_device(&mac.access_token, &tv.device_id, " TV du salon ")
            .await
            .unwrap();

        let list = devices.list_devices(&mac.access_token).await.unwrap();

        assert_eq!(
            list.iter().find(|d| d.id == tv.device_id).unwrap().name,
            "TV du salon"
        );

        assert!(matches!(
            devices
                .forget_device(&mac.access_token, &tv.device_id)
                .await,
            Err(NorylumError::Validation(_))
        ));

        let child = auth
            .create_profile(&mac.account_id, "Junior", true, Some(10))
            .await
            .unwrap();

        let child_session = auth
            .switch_profile(&mac.access_token, &child)
            .await
            .unwrap();

        assert!(matches!(
            devices
                .rename_device(&child_session.access_token, &tv.device_id, "Interdit")
                .await,
            Err(NorylumError::Forbidden(_))
        ));

        devices
            .revoke_device(&mac.access_token, &tv.device_id)
            .await
            .unwrap();

        devices
            .forget_device(&mac.access_token, &tv.device_id)
            .await
            .unwrap();

        assert!(auth.authenticate(&tv.access_token).await.is_err());

        assert!(auth.authenticate(&foreign.access_token).await.is_ok());
    }

    #[tokio::test]
    async fn test_cleanup_expired_sessions_per_account() {
        let db = Database::in_memory().unwrap();
        let auth = AuthService::new(db.clone());
        let devices = DeviceService::new(db.clone());

        auth.bootstrap_admin("admin", PASSWORD).await.unwrap();

        let current = auth
            .login_with_device("admin", PASSWORD, Some("Actuel"))
            .await
            .unwrap();

        let expired = auth
            .login_with_device("admin", PASSWORD, Some("Ancien"))
            .await
            .unwrap();

        add_other_account(&db).await;

        let foreign = auth
            .login_with_device("other", PASSWORD, Some("Autre"))
            .await
            .unwrap();

        db.with_connection(|conn| {
            for session in [expired.session_id.as_str(), foreign.session_id.as_str()] {
                conn.execute(
                    "UPDATE auth_sessions
                     SET expires_at = 1
                     WHERE session_id = ?1",
                    params![session],
                )
                .map_err(storage)?;
            }

            for device in [&expired.device_id, &foreign.device_id] {
                conn.execute(
                    "UPDATE devices
                     SET last_seen_at = 1
                     WHERE id = ?1",
                    params![device.as_str()],
                )
                .map_err(storage)?;
            }

            Ok(())
        })
        .await
        .unwrap();

        let report = devices.maintenance(&current.access_token).await.unwrap();

        assert_eq!(report.expired_sessions_removed, 1);
        assert_eq!(report.inactive_devices_removed, 1);

        let remaining = devices.list_devices(&current.access_token).await.unwrap();

        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, current.device_id);

        let (foreign_session, foreign_device) = db
            .with_connection(|conn| {
                let count_session: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM auth_sessions
                     WHERE session_id = ?1",
                        params![foreign.session_id.as_str()],
                        |row| row.get(0),
                    )
                    .map_err(storage)?;

                let count_device: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM devices
                     WHERE id = ?1",
                        params![foreign.device_id.as_str()],
                        |row| row.get(0),
                    )
                    .map_err(storage)?;

                Ok((count_session, count_device))
            })
            .await
            .unwrap();

        assert_eq!(foreign_session, 1);
        assert_eq!(foreign_device, 1);

        let repeated = devices.maintenance(&current.access_token).await.unwrap();

        assert_eq!(repeated.expired_sessions_removed, 0);
        assert_eq!(repeated.inactive_devices_removed, 0);
    }
}
