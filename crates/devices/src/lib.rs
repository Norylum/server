//! Appareils et sessions Norylum.
//! Isolation par compte, sessions revocables et controle du profil principal.

use chrono::Utc;
use norylum_foundation::{DeviceId, NorylumError, ProfileId};
use norylum_persistence::Database;
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;
use sha2::{Digest, Sha256};

fn storage(error: impl std::fmt::Display) -> NorylumError {
    NorylumError::Internal(format!("Erreur devices SQLite : {error}"))
}

fn unauthorized() -> NorylumError {
    NorylumError::Unauthorized("Session invalide".into())
}

fn token_hash(token: &str) -> Result<String, NorylumError> {
    if token.len() != 64 || !token.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(unauthorized());
    }

    Ok(format!("{:x}", Sha256::digest(token.as_bytes())))
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceRecord {
    pub id: DeviceId,
    pub name: String,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub active_sessions: u64,
    pub is_current: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionRecord {
    pub id: String,
    pub device_id: DeviceId,
    pub device_name: String,
    pub profile_id: ProfileId,
    pub expires_at: i64,
    pub is_current: bool,
}

#[derive(Clone)]
pub struct DeviceService {
    db: Database,
}

impl DeviceService {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Revalide le jeton en base et exige le profil principal.
    /// Ne fait pas confiance aux identifiants HTTP du client.
    fn principal(
        tx: &Transaction<'_>,
        token: &str,
        now: i64,
    ) -> Result<(String, String, String), NorylumError> {
        let hash = token_hash(token)?;

        let source: Option<(String, String, String, String, i64)> = tx
            .query_row(
                "SELECT s.account_id, s.profile_id,
                        s.device_id, s.session_id, p.is_child
                 FROM auth_sessions s
                 JOIN profiles p
                   ON p.id = s.profile_id
                  AND p.account_id = s.account_id
                 JOIN devices d
                   ON d.id = s.device_id
                  AND d.account_id = s.account_id
                 WHERE s.token_hash = ?1
                   AND s.expires_at > ?2",
                params![hash, now],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()
            .map_err(storage)?;

        let (account, profile, device, session, child) = source.ok_or_else(unauthorized)?;

        if child != 0 {
            return Err(NorylumError::Forbidden("Profil principal requis".into()));
        }

        let principal: Option<String> = tx
            .query_row(
                "SELECT id FROM profiles
             WHERE account_id = ?1
             ORDER BY rowid ASC LIMIT 1",
                params![account],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;

        if principal.as_deref() != Some(profile.as_str()) {
            return Err(NorylumError::Forbidden(
                "Seul le profil principal peut gerer les appareils".into(),
            ));
        }

        Ok((account, device, session))
    }

    pub async fn list_devices(&self, token: &str) -> Result<Vec<DeviceRecord>, NorylumError> {
        let now = Utc::now().timestamp();

        self.db
            .with_connection(|conn| {
                let tx = conn.transaction().map_err(storage)?;
                let (account, current_device, _) = Self::principal(&tx, token, now)?;

                let mut statement = tx
                    .prepare(
                        "SELECT d.id, d.name, d.created_at,
                        d.last_seen_at, COUNT(s.token_hash)
                 FROM devices d
                 LEFT JOIN auth_sessions s
                   ON s.device_id = d.id
                  AND s.account_id = d.account_id
                  AND s.expires_at > ?2
                 WHERE d.account_id = ?1
                 GROUP BY d.id, d.name,
                          d.created_at, d.last_seen_at
                 ORDER BY d.created_at DESC, d.id",
                    )
                    .map_err(storage)?;

                let rows = statement
                    .query_map(params![account, now], |row| {
                        let id: String = row.get(0)?;
                        let count: i64 = row.get(4)?;

                        Ok(DeviceRecord {
                            is_current: id == current_device,
                            id: DeviceId::new(id),
                            name: row.get(1)?,
                            created_at: row.get(2)?,
                            last_seen_at: row.get(3)?,
                            active_sessions: count.max(0) as u64,
                        })
                    })
                    .map_err(storage)?;

                rows.collect::<Result<Vec<_>, _>>().map_err(storage)
            })
            .await
    }

    pub async fn list_sessions(&self, token: &str) -> Result<Vec<SessionRecord>, NorylumError> {
        let now = Utc::now().timestamp();

        self.db
            .with_connection(|conn| {
                let tx = conn.transaction().map_err(storage)?;
                let (account, _, current_session) = Self::principal(&tx, token, now)?;

                let mut statement = tx
                    .prepare(
                        "SELECT s.session_id, s.device_id,
                        d.name, s.profile_id, s.expires_at
                 FROM auth_sessions s
                 JOIN devices d
                   ON d.id = s.device_id
                  AND d.account_id = s.account_id
                 WHERE s.account_id = ?1
                   AND s.expires_at > ?2
                 ORDER BY s.expires_at DESC, s.session_id",
                    )
                    .map_err(storage)?;

                let rows = statement
                    .query_map(params![account, now], |row| {
                        let id: String = row.get(0)?;

                        Ok(SessionRecord {
                            is_current: id == current_session,
                            id,
                            device_id: DeviceId::new(row.get::<_, String>(1)?),
                            device_name: row.get(2)?,
                            profile_id: ProfileId::new(row.get::<_, String>(3)?),
                            expires_at: row.get(4)?,
                        })
                    })
                    .map_err(storage)?;

                rows.collect::<Result<Vec<_>, _>>().map_err(storage)
            })
            .await
    }

    /// Revoque toutes les sessions d'un appareil du compte.
    /// L'appareil reste dans l'historique avec zero session.
    pub async fn revoke_device(
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
                        params![device_id.as_str(), account],
                        |row| row.get(0),
                    )
                    .map_err(storage)?;

                if exists == 0 {
                    return Err(NorylumError::NotFound("Appareil introuvable".into()));
                }

                tx.execute(
                    "DELETE FROM auth_sessions
                 WHERE device_id = ?1 AND account_id = ?2",
                    params![device_id.as_str(), account],
                )
                .map_err(storage)?;

                tx.commit().map_err(storage)?;
                Ok(())
            })
            .await
    }

    /// Revoque une seule session, uniquement dans le compte
    /// du proprietaire de la session appelante.
    pub async fn revoke_session(&self, token: &str, session_id: &str) -> Result<(), NorylumError> {
        let now = Utc::now().timestamp();

        self.db
            .with_connection(|conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(storage)?;

                let (account, _, _) = Self::principal(&tx, token, now)?;

                let deleted = tx
                    .execute(
                        "DELETE FROM auth_sessions
                 WHERE session_id = ?1
                   AND account_id = ?2",
                        params![session_id, account],
                    )
                    .map_err(storage)?;

                if deleted == 0 {
                    return Err(NorylumError::NotFound("Session introuvable".into()));
                }

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
    use norylum_foundation::AccountId;

    const PASSWORD: &str = "devices-test-password-2026";

    #[tokio::test]
    async fn test_device_sessions_and_account_isolation() {
        let db = Database::in_memory().unwrap();
        let auth = AuthService::new(db.clone());
        let devices = DeviceService::new(db.clone());

        auth.bootstrap_admin("admin", PASSWORD).await.unwrap();

        let first = auth
            .login_with_device("admin", PASSWORD, Some("Mac"))
            .await
            .unwrap();

        let second = auth
            .login_with_device("admin", PASSWORD, Some("TV salon"))
            .await
            .unwrap();

        assert_ne!(first.device_id, second.device_id);
        assert_ne!(first.session_id, second.session_id);

        let child = auth
            .create_profile(&first.account_id, "Junior", true, Some(10))
            .await
            .unwrap();

        let switched = auth
            .switch_profile(&first.access_token, &child)
            .await
            .unwrap();

        assert_eq!(switched.device_id, first.device_id);
        assert_ne!(switched.session_id, first.session_id);

        let child_denied = devices.list_sessions(&switched.access_token).await;

        assert!(matches!(child_denied, Err(NorylumError::Forbidden(_))));

        let list = devices.list_devices(&first.access_token).await.unwrap();

        assert_eq!(list.len(), 2);

        let mac = list.iter().find(|d| d.id == first.device_id).unwrap();

        assert_eq!(mac.active_sessions, 2);
        assert!(mac.is_current);

        let sessions = devices.list_sessions(&first.access_token).await.unwrap();

        assert_eq!(sessions.len(), 3);
        assert!(sessions.iter().any(|s| s.is_current));

        // Deuxieme compte, meme base, aucune isolation implicite
        // par client : les requetes doivent filtrer en SQLite.
        let foreign_account = AccountId::random();
        let foreign_profile = ProfileId::random();

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
                params![foreign_account.as_str(), encoded],
            )
            .map_err(storage)?;

            conn.execute(
                "INSERT INTO profiles
                 (id, account_id, name, is_child, age_rating_max)
                 VALUES (?1, ?2, 'Principal', 0, NULL)",
                params![foreign_profile.as_str(), foreign_account.as_str()],
            )
            .map_err(storage)?;

            Ok(())
        })
        .await
        .unwrap();

        let foreign = auth.login("other", PASSWORD).await.unwrap();

        assert!(matches!(
            devices
                .revoke_device(&first.access_token, &foreign.device_id)
                .await,
            Err(NorylumError::NotFound(_))
        ));

        assert!(matches!(
            devices
                .revoke_session(&first.access_token, &foreign.session_id)
                .await,
            Err(NorylumError::NotFound(_))
        ));

        assert_eq!(
            devices
                .list_devices(&foreign.access_token)
                .await
                .unwrap()
                .len(),
            1
        );

        devices
            .revoke_session(&first.access_token, &second.session_id)
            .await
            .unwrap();

        assert!(auth.authenticate(&second.access_token).await.is_err());

        assert!(auth.authenticate(&first.access_token).await.is_ok());

        devices
            .revoke_device(&first.access_token, &first.device_id)
            .await
            .unwrap();

        assert!(auth.authenticate(&first.access_token).await.is_err());

        assert!(auth.authenticate(&switched.access_token).await.is_err());

        // La revocation du compte admin n'affecte pas l'autre.
        assert!(auth.authenticate(&foreign.access_token).await.is_ok());
    }
}
