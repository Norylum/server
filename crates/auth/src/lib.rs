//! Authentification locale Norylum.
//! Argon2id, sessions opaques et persistance SQLite.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use chrono::Utc;
use norylum_foundation::{AccountId, DeviceId, NorylumError, ProfileId, RequestContext};
use norylum_persistence::Database;
use rand_core::OsRng;
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

const SESSION_LIFETIME: i64 = 7 * 24 * 60 * 60;

fn storage(error: impl std::fmt::Display) -> NorylumError {
    NorylumError::Internal(format!("Erreur auth SQLite : {error}"))
}

fn rejected() -> NorylumError {
    NorylumError::Unauthorized("Identifiants ou session invalides".into())
}

fn digest(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

#[derive(Debug, Clone, Serialize)]
pub struct LoginResponse {
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: u64,
    pub account_id: AccountId,
    pub profile_id: ProfileId,
}

#[derive(Clone)]
pub struct AuthService {
    db: Database,
}

impl AuthService {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    pub async fn bootstrap_admin(
        &self,
        username: &str,
        password: &str,
    ) -> Result<bool, NorylumError> {
        if !(3..=64).contains(&username.len())
            || !username
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(NorylumError::Validation(
                "Identifiant invalide (3-64 caracteres ASCII)".into(),
            ));
        }

        if !(12..=1024).contains(&password.len()) {
            return Err(NorylumError::Validation(
                "Mot de passe : 12 a 1024 octets requis".into(),
            ));
        }

        let salt = SaltString::generate(&mut OsRng);
        let password_hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|_| NorylumError::Internal("Echec du hachage du mot de passe".into()))?
            .to_string();

        let account = AccountId::random();
        let profile = ProfileId::random();

        self.db
            .with_connection(|conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(storage)?;

                let count: i64 = tx
                    .query_row("SELECT COUNT(*) FROM accounts", [], |row| row.get(0))
                    .map_err(storage)?;

                if count != 0 {
                    return Ok(false);
                }

                tx.execute(
                    "INSERT INTO accounts
                 (id, username, password_hash)
                 VALUES (?1, ?2, ?3)",
                    params![account.as_str(), username, password_hash],
                )
                .map_err(storage)?;

                tx.execute(
                    "INSERT INTO profiles
                 (id, account_id, name, is_child, age_rating_max)
                 VALUES (?1, ?2, 'Principal', 0, NULL)",
                    params![profile.as_str(), account.as_str()],
                )
                .map_err(storage)?;

                tx.commit().map_err(storage)?;
                Ok(true)
            })
            .await
    }

    pub async fn login(
        &self,
        username: &str,
        password: &str,
    ) -> Result<LoginResponse, NorylumError> {
        let account: Option<(String, String)> = self
            .db
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT id, password_hash FROM accounts
                     WHERE username = ?1 COLLATE NOCASE",
                    params![username],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(storage)
            })
            .await?;

        let (id, encoded) = account.ok_or_else(rejected)?;

        {
            let parsed = PasswordHash::new(&encoded).map_err(|_| rejected())?;

            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .map_err(|_| rejected())?;
        }

        let account_id = AccountId::new(id);

        let profile_id: String = self
            .db
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT id FROM profiles
                 WHERE account_id = ?1
                 ORDER BY rowid ASC LIMIT 1",
                    params![account_id.as_str()],
                    |row| row.get(0),
                )
                .map_err(storage)
            })
            .await?;

        self.issue_session(&account_id, &ProfileId::new(profile_id))
            .await
    }

    async fn issue_session(
        &self,
        account_id: &AccountId,
        profile_id: &ProfileId,
    ) -> Result<LoginResponse, NorylumError> {
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());

        let token_hash = digest(&token);
        let expires_at = Utc::now().timestamp() + SESSION_LIFETIME;

        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO auth_sessions
                 (token_hash, account_id, profile_id, expires_at)
                 VALUES (?1, ?2, ?3, ?4)",
                    params![
                        token_hash,
                        account_id.as_str(),
                        profile_id.as_str(),
                        expires_at
                    ],
                )
                .map_err(storage)?;
                Ok(())
            })
            .await?;

        Ok(LoginResponse {
            access_token: token,
            token_type: "Bearer",
            expires_in: SESSION_LIFETIME as u64,
            account_id: account_id.clone(),
            profile_id: profile_id.clone(),
        })
    }

    pub async fn authenticate(&self, token: &str) -> Result<RequestContext, NorylumError> {
        if token.len() != 64 || !token.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(rejected());
        }

        let token_hash = digest(token);
        let now = Utc::now().timestamp();

        let session: Option<(String, String, i64, Option<i64>)> = self
            .db
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT s.account_id, s.profile_id,
                            p.is_child, p.age_rating_max
                     FROM auth_sessions s
                     JOIN profiles p
                       ON p.id = s.profile_id
                      AND p.account_id = s.account_id
                     WHERE s.token_hash = ?1
                       AND s.expires_at > ?2",
                    params![token_hash, now],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(storage)
            })
            .await?;

        let (account, profile, is_child, age_max) = session.ok_or_else(rejected)?;

        Ok(RequestContext::authenticated(
            AccountId::new(account),
            ProfileId::new(profile),
            DeviceId::new("dev_authenticated"),
            is_child != 0,
            age_max.and_then(|v| u32::try_from(v).ok()),
        ))
    }

    pub async fn logout(&self, token: &str) -> Result<(), NorylumError> {
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "DELETE FROM auth_sessions WHERE token_hash = ?1",
                    params![digest(token)],
                )
                .map_err(storage)?;
                Ok(())
            })
            .await
    }

    // Reserve aux services internes.
    pub async fn create_profile(
        &self,
        account_id: &AccountId,
        name: &str,
        is_child: bool,
        max_age: Option<u32>,
    ) -> Result<ProfileId, NorylumError> {
        if name.trim().is_empty() || name.len() > 64 {
            return Err(NorylumError::Validation("Nom de profil invalide".into()));
        }

        let id = ProfileId::random();

        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO profiles
                 (id, account_id, name, is_child, age_rating_max)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        id.as_str(),
                        account_id.as_str(),
                        name.trim(),
                        is_child as i64,
                        max_age.map(i64::from),
                    ],
                )
                .map_err(storage)?;
                Ok(())
            })
            .await?;

        Ok(id)
    }

    pub async fn switch_profile(
        &self,
        token: &str,
        profile_id: &ProfileId,
    ) -> Result<LoginResponse, NorylumError> {
        self.switch_profile_with_password(token, profile_id, None)
            .await
    }

    /// Le mot de passe du compte est obligatoire
    /// lorsque le profil cible est adulte.
    ///
    /// La verification du jeton source, du profil cible
    /// et la creation de session forment une transaction.
    pub async fn switch_profile_with_password(
        &self,
        token: &str,
        profile_id: &ProfileId,
        account_password: Option<&str>,
    ) -> Result<LoginResponse, NorylumError> {
        if token.len() != 64 || !token.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(rejected());
        }

        let incoming_hash = digest(token);
        let new_token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let new_hash = digest(&new_token);
        let now = Utc::now().timestamp();
        let expires_at = now + SESSION_LIFETIME;

        let account_id = self
            .db
            .with_connection(|conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(storage)?;

                let source: Option<(String, i64)> = tx
                    .query_row(
                        "SELECT s.account_id, p.is_child
                     FROM auth_sessions s
                     JOIN profiles p
                       ON p.id = s.profile_id
                      AND p.account_id = s.account_id
                     WHERE s.token_hash = ?1
                       AND s.expires_at > ?2",
                        params![incoming_hash, now],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(storage)?;

                let (account, source_child) = source.ok_or_else(rejected)?;

                if source_child != 0 {
                    return Err(NorylumError::Forbidden(
                        "Changement de profil interdit aux enfants".into(),
                    ));
                }

                let target_child: Option<i64> = tx
                    .query_row(
                        "SELECT is_child FROM profiles
                     WHERE id = ?1 AND account_id = ?2",
                        params![profile_id.as_str(), account],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(storage)?;

                let target_child = target_child
                    .ok_or_else(|| NorylumError::Forbidden("Profil inaccessible".into()))?;

                if target_child == 0 {
                    let password = account_password.ok_or_else(|| {
                        NorylumError::Forbidden(
                            "Mot de passe du compte requis pour un profil adulte".into(),
                        )
                    })?;

                    let encoded: String = tx
                        .query_row(
                            "SELECT password_hash FROM accounts
                         WHERE id = ?1",
                            params![account],
                            |row| row.get(0),
                        )
                        .map_err(storage)?;

                    let parsed = PasswordHash::new(&encoded).map_err(|_| rejected())?;

                    Argon2::default()
                        .verify_password(password.as_bytes(), &parsed)
                        .map_err(|_| {
                            NorylumError::Forbidden("Mot de passe du compte invalide".into())
                        })?;
                }

                tx.execute(
                    "INSERT INTO auth_sessions
                 (token_hash, account_id, profile_id, expires_at)
                 VALUES (?1, ?2, ?3, ?4)",
                    params![new_hash, account, profile_id.as_str(), expires_at,],
                )
                .map_err(storage)?;

                tx.commit().map_err(storage)?;
                Ok(AccountId::new(account))
            })
            .await?;

        Ok(LoginResponse {
            access_token: new_token,
            token_type: "Bearer",
            expires_in: SESSION_LIFETIME as u64,
            account_id,
            profile_id: profile_id.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_auth_lifecycle() {
        let auth = AuthService::new(Database::in_memory().unwrap());

        assert!(auth
            .bootstrap_admin("admin", "test-password-2026-securise")
            .await
            .unwrap());

        assert!(!auth
            .bootstrap_admin("second", "test-password-2026-securise")
            .await
            .unwrap());

        assert!(matches!(
            auth.login("admin", "incorrect").await,
            Err(NorylumError::Unauthorized(_))
        ));

        let session = auth
            .login("admin", "test-password-2026-securise")
            .await
            .unwrap();

        let ctx = auth.authenticate(&session.access_token).await.unwrap();

        assert!(!ctx.is_child());

        let child = auth
            .create_profile(&session.account_id, "Enfant", true, Some(10))
            .await
            .unwrap();

        let child_session = auth
            .switch_profile(&session.access_token, &child)
            .await
            .unwrap();

        let child_ctx = auth
            .authenticate(&child_session.access_token)
            .await
            .unwrap();

        assert!(child_ctx.is_child());
        assert_eq!(child_ctx.age_rating_max, Some(10));

        assert!(matches!(
            auth.switch_profile(&child_session.access_token, &session.profile_id)
                .await,
            Err(NorylumError::Forbidden(_))
        ));

        auth.logout(&session.access_token).await.unwrap();

        assert!(matches!(
            auth.authenticate(&session.access_token).await,
            Err(NorylumError::Unauthorized(_))
        ));
    }
}

#[cfg(test)]
mod profile_switch_tests;
