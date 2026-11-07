//! Codes PIN de profils : hash Argon2id et verrouillage SQLite.

use super::*;
use rusqlite::Transaction;

const MAX_PIN_FAILURES: i64 = 5;
const PIN_LOCK_SECONDS: i64 = 15 * 60;

fn valid_pin(pin: &str) -> bool {
    (6..=8).contains(&pin.len()) && pin.bytes().all(|c| c.is_ascii_digit())
}

fn password_matches(encoded: &str, provided: &str) -> bool {
    if provided.len() > 1024 {
        return false;
    }

    match PasswordHash::new(encoded) {
        Ok(parsed) => Argon2::default()
            .verify_password(provided.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

fn token_valid(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|c| c.is_ascii_hexdigit())
}

/// Verifie qu'une session appartient au profil principal
/// et que le mot de passe du compte est valide.
fn principal_account(
    tx: &Transaction<'_>,
    token: &str,
    password: &str,
    now: i64,
) -> Result<String, NorylumError> {
    if !token_valid(token) {
        return Err(rejected());
    }

    let current: Option<(String, String, i64)> = tx
        .query_row(
            "SELECT s.account_id, s.profile_id, p.is_child
             FROM auth_sessions s
             JOIN profiles p
               ON p.id = s.profile_id
              AND p.account_id = s.account_id
             WHERE s.token_hash = ?1
               AND s.expires_at > ?2",
            params![digest(token), now],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(storage)?;

    let (account, profile, child) = current.ok_or_else(rejected)?;

    if child != 0 {
        return Err(NorylumError::Forbidden("Profil principal requis".into()));
    }

    let principal: String = tx
        .query_row(
            "SELECT id FROM profiles
             WHERE account_id = ?1
             ORDER BY rowid ASC LIMIT 1",
            params![account],
            |row| row.get(0),
        )
        .map_err(storage)?;

    if principal != profile {
        return Err(NorylumError::Forbidden("Profil principal requis".into()));
    }

    let stored_hash: String = tx
        .query_row(
            "SELECT password_hash FROM accounts WHERE id = ?1",
            params![account],
            |row| row.get(0),
        )
        .map_err(storage)?;

    if !password_matches(&stored_hash, password) {
        return Err(NorylumError::Forbidden("Mot de passe invalide".into()));
    }

    Ok(account)
}

impl AuthService {
    /// Configure ou remplace le PIN d'un profil adulte.
    pub async fn set_profile_pin(
        &self,
        token: &str,
        profile_id: &ProfileId,
        account_password: &str,
        pin: &str,
    ) -> Result<(), NorylumError> {
        if !valid_pin(pin) {
            return Err(NorylumError::Validation(
                "PIN : 6 a 8 chiffres requis".into(),
            ));
        }

        let salt = SaltString::generate(&mut OsRng);
        let encoded = Argon2::default()
            .hash_password(pin.as_bytes(), &salt)
            .map_err(|_| NorylumError::Internal("Echec du hachage PIN".into()))?
            .to_string();

        self.change_profile_pin(token, profile_id, account_password, Some(encoded))
            .await
    }

    /// Supprime un PIN, avec confirmation du mot de passe.
    pub async fn delete_profile_pin(
        &self,
        token: &str,
        profile_id: &ProfileId,
        account_password: &str,
    ) -> Result<(), NorylumError> {
        self.change_profile_pin(token, profile_id, account_password, None)
            .await
    }

    async fn change_profile_pin(
        &self,
        token: &str,
        profile_id: &ProfileId,
        account_password: &str,
        pin_hash: Option<String>,
    ) -> Result<(), NorylumError> {
        let now = Utc::now().timestamp();

        self.db
            .with_connection(|conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(storage)?;

                let account = principal_account(&tx, token, account_password, now)?;

                let is_child: Option<i64> = tx
                    .query_row(
                        "SELECT is_child FROM profiles
                         WHERE id = ?1 AND account_id = ?2",
                        params![profile_id.as_str(), account],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(storage)?;

                match is_child {
                    None => return Err(NorylumError::NotFound("Profil introuvable".into())),
                    _ => {}
                }

                tx.execute(
                    "UPDATE profiles
                     SET pin_hash = ?3,
                         pin_failures = 0,
                         pin_locked_until = 0
                     WHERE id = ?1 AND account_id = ?2",
                    params![profile_id.as_str(), account, pin_hash,],
                )
                .map_err(storage)?;

                // Un jeton actif avant le changement ne conserve
                // pas implicitement son droit d'acces.
                tx.execute(
                    "DELETE FROM auth_sessions
                     WHERE account_id = ?1 AND profile_id = ?2",
                    params![account, profile_id.as_str()],
                )
                .map_err(storage)?;

                tx.commit().map_err(storage)?;
                Ok(())
            })
            .await
    }

    /// Accede a un profil :
    /// - enfant : session adulte source suffisante ;
    /// - adulte sans PIN : mot de passe du compte ;
    /// - adulte avec PIN : PIN ou mot de passe du compte.
    pub async fn switch_profile_with_credentials(
        &self,
        token: &str,
        profile_id: &ProfileId,
        account_password: Option<&str>,
        profile_pin: Option<&str>,
    ) -> Result<LoginResponse, NorylumError> {
        if !token_valid(token) {
            return Err(rejected());
        }

        if account_password.is_some() && profile_pin.is_some() {
            return Err(NorylumError::Validation(
                "Fournir soit account_password, soit profile_pin".into(),
            ));
        }

        let incoming_hash = digest(token);
        let new_token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let new_hash = digest(&new_token);
        let session_id = format!("ses_{}", Uuid::new_v4().simple());

        let now = Utc::now().timestamp();
        let expires_at = now + SESSION_LIFETIME;

        let (account_id, device_id) = self
            .db
            .with_connection(|conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(storage)?;

                let source: Option<(String, i64, String)> = tx
                    .query_row(
                        "SELECT s.account_id, p.is_child, s.device_id
                         FROM auth_sessions s
                         JOIN devices d
                           ON d.id = s.device_id
                          AND d.account_id = s.account_id
                         JOIN profiles p
                           ON p.id = s.profile_id
                          AND p.account_id = s.account_id
                         WHERE s.token_hash = ?1
                           AND s.expires_at > ?2",
                        params![incoming_hash, now],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()
                    .map_err(storage)?;

                let (account, source_child, device_id) = source.ok_or_else(rejected)?;

                if source_child != 0 {
                    return Err(NorylumError::Forbidden(
                        "Changement de profil interdit aux enfants".into(),
                    ));
                }

                let target: Option<(i64, Option<String>, i64, i64)> = tx
                    .query_row(
                        "SELECT is_child, pin_hash,
                                pin_failures, pin_locked_until
                         FROM profiles
                         WHERE id = ?1 AND account_id = ?2",
                        params![profile_id.as_str(), account],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                    .optional()
                    .map_err(storage)?;

                let (target_child, target_pin_hash, failures, locked_until) =
                    target.ok_or_else(|| NorylumError::Forbidden("Profil inaccessible".into()))?;

                if target_child == 0 {
                    if let Some(password) = account_password {
                        let encoded: String = tx
                            .query_row(
                                "SELECT password_hash FROM accounts
                                 WHERE id = ?1",
                                params![account],
                                |row| row.get(0),
                            )
                            .map_err(storage)?;

                        if !password_matches(&encoded, password) {
                            return Err(NorylumError::Forbidden("Mot de passe invalide".into()));
                        }

                        // Recuperation par le proprietaire du compte.
                        tx.execute(
                            "UPDATE profiles
                             SET pin_failures = 0,
                                 pin_locked_until = 0
                             WHERE id = ?1 AND account_id = ?2",
                            params![profile_id.as_str(), account],
                        )
                        .map_err(storage)?;
                    } else if let Some(encoded) = target_pin_hash {
                        let pin = profile_pin.ok_or_else(|| {
                            NorylumError::Forbidden("PIN ou mot de passe requis".into())
                        })?;

                        if locked_until > now {
                            return Err(NorylumError::Forbidden(
                                "PIN temporairement verrouille".into(),
                            ));
                        }

                        let accepted = valid_pin(pin) && password_matches(&encoded, pin);

                        if !accepted {
                            let next = if locked_until > 0 && locked_until <= now {
                                1
                            } else {
                                failures.saturating_add(1)
                            };

                            let next_lock = if next >= MAX_PIN_FAILURES {
                                now + PIN_LOCK_SECONDS
                            } else {
                                0
                            };

                            tx.execute(
                                "UPDATE profiles
                                 SET pin_failures = ?3,
                                     pin_locked_until = ?4
                                 WHERE id = ?1 AND account_id = ?2",
                                params![profile_id.as_str(), account, next, next_lock,],
                            )
                            .map_err(storage)?;

                            // Les epreuves incorrectes DOIVENT etre
                            // persistees, meme si l'API renvoie 403.
                            tx.commit().map_err(storage)?;

                            return Err(NorylumError::Forbidden(
                                "PIN incorrect ou verrouille".into(),
                            ));
                        }

                        tx.execute(
                            "UPDATE profiles
                             SET pin_failures = 0,
                                 pin_locked_until = 0
                             WHERE id = ?1 AND account_id = ?2",
                            params![profile_id.as_str(), account],
                        )
                        .map_err(storage)?;
                    } else {
                        return Err(NorylumError::Forbidden(
                            "Mot de passe du compte requis".into(),
                        ));
                    }
                } else if let Some(encoded) = target_pin_hash {
                    // Profil enfant avec PIN : verification requise.
                    let pin = profile_pin.ok_or_else(|| {
                        NorylumError::Forbidden("PIN requis pour ce profil enfant".into())
                    })?;

                    if locked_until > now {
                        return Err(NorylumError::Forbidden(
                            "PIN temporairement verrouille".into(),
                        ));
                    }

                    let accepted = valid_pin(pin) && password_matches(&encoded, pin);

                    if !accepted {
                        let next = if locked_until > 0 && locked_until <= now {
                            1
                        } else {
                            failures.saturating_add(1)
                        };

                        let next_lock = if next >= MAX_PIN_FAILURES {
                            now + PIN_LOCK_SECONDS
                        } else {
                            0
                        };

                        tx.execute(
                            "UPDATE profiles
                             SET pin_failures = ?3,
                                 pin_locked_until = ?4
                             WHERE id = ?1 AND account_id = ?2",
                            params![profile_id.as_str(), account, next, next_lock,],
                        )
                        .map_err(storage)?;

                        tx.commit().map_err(storage)?;

                        return Err(NorylumError::Forbidden(
                            "PIN incorrect ou verrouille".into(),
                        ));
                    }

                    tx.execute(
                        "UPDATE profiles
                         SET pin_failures = 0,
                             pin_locked_until = 0
                         WHERE id = ?1 AND account_id = ?2",
                        params![profile_id.as_str(), account],
                    )
                    .map_err(storage)?;
                }
                // Profil enfant sans PIN : acces direct (comportement inchange).

                // Creation atomique apres verification des droits.
                tx.execute(
                    "INSERT INTO auth_sessions
                     (token_hash, account_id, profile_id,
                      expires_at, device_id, session_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        new_hash,
                        account,
                        profile_id.as_str(),
                        expires_at,
                        device_id,
                        session_id,
                    ],
                )
                .map_err(storage)?;

                tx.execute(
                    "UPDATE devices
                     SET last_seen_at = ?3
                     WHERE id = ?1 AND account_id = ?2",
                    params![device_id, account, now],
                )
                .map_err(storage)?;

                tx.commit().map_err(storage)?;
                Ok((AccountId::new(account), DeviceId::new(device_id)))
            })
            .await?;

        Ok(LoginResponse {
            access_token: new_token,
            token_type: "Bearer",
            expires_in: SESSION_LIFETIME as u64,
            account_id,
            profile_id: profile_id.clone(),
            device_id,
            session_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWORD: &str = "norylum-unit-test-password-2026";
    const PIN: &str = "829461";

    #[tokio::test]
    async fn test_pin_lifecycle_and_lockout() {
        let auth = AuthService::new(Database::in_memory().unwrap());

        auth.bootstrap_admin("admin", PASSWORD).await.unwrap();

        let admin = auth.login("admin", PASSWORD).await.unwrap();

        let adult = auth
            .create_profile(&admin.account_id, "Secondaire", false, None)
            .await
            .unwrap();

        let child = auth
            .create_profile(&admin.account_id, "Junior", true, Some(10))
            .await
            .unwrap();

        assert!(matches!(
            auth.set_profile_pin(&admin.access_token, &adult, PASSWORD, "123",)
                .await,
            Err(NorylumError::Validation(_))
        ));

        assert!(matches!(
            auth.set_profile_pin(&admin.access_token, &adult, "incorrect", PIN,)
                .await,
            Err(NorylumError::Forbidden(_))
        ));

        let existing = auth
            .switch_profile_with_password(&admin.access_token, &adult, Some(PASSWORD))
            .await
            .unwrap();

        auth.set_profile_pin(&admin.access_token, &adult, PASSWORD, PIN)
            .await
            .unwrap();

        assert!(auth.authenticate(&existing.access_token).await.is_err());

        let admitted = auth
            .switch_profile_with_credentials(&admin.access_token, &adult, None, Some(PIN))
            .await
            .unwrap();

        assert_eq!(admitted.profile_id, adult);

        for _ in 0..5 {
            assert!(matches!(
                auth.switch_profile_with_credentials(
                    &admin.access_token,
                    &adult,
                    None,
                    Some("000000"),
                )
                .await,
                Err(NorylumError::Forbidden(_))
            ));
        }

        // Le bon PIN reste refuse pendant le verrouillage.
        assert!(matches!(
            auth.switch_profile_with_credentials(&admin.access_token, &adult, None, Some(PIN),)
                .await,
            Err(NorylumError::Forbidden(_))
        ));

        // Recuperation par le mot de passe du compte.
        let recovered = auth
            .switch_profile_with_credentials(&admin.access_token, &adult, Some(PASSWORD), None)
            .await
            .unwrap();

        let ctx = auth.authenticate(&recovered.access_token).await.unwrap();

        assert!(!ctx.is_child());

        auth.delete_profile_pin(&admin.access_token, &adult, PASSWORD)
            .await
            .unwrap();

        assert!(auth.authenticate(&recovered.access_token).await.is_err());

        // Sans PIN configure, il faut le mot de passe.
        assert!(auth
            .switch_profile_with_credentials(&admin.access_token, &adult, None, Some(PIN),)
            .await
            .is_err());

        assert!(auth
            .switch_profile_with_password(&admin.access_token, &adult, Some(PASSWORD),)
            .await
            .is_ok());

        // Un enfant ne peut pas changer de profil.
        let child_session = auth
            .switch_profile(&admin.access_token, &child)
            .await
            .unwrap();

        assert!(matches!(
            auth.switch_profile_with_credentials(
                &child_session.access_token,
                &adult,
                Some(PASSWORD),
                None,
            )
            .await,
            Err(NorylumError::Forbidden(_))
        ));
    }

    #[tokio::test]
    async fn test_child_pin_lifecycle_and_lockout() {
        let auth = AuthService::new(Database::in_memory().unwrap());
        auth.bootstrap_admin("admin", PASSWORD).await.unwrap();
        let admin = auth.login("admin", PASSWORD).await.unwrap();

        let child = auth
            .create_profile(&admin.account_id, "Junior", true, Some(8))
            .await
            .unwrap();

        // Sans PIN : acces direct depuis un profil adulte.
        let no_pin = auth
            .switch_profile(&admin.access_token, &child)
            .await
            .unwrap();
        assert_eq!(no_pin.profile_id, child);

        // Pose du PIN enfant par le profil principal.
        auth.set_profile_pin(&admin.access_token, &child, PASSWORD, PIN)
            .await
            .unwrap();

        // Avec PIN : acces direct sans credentials refuse.
        assert!(matches!(
            auth.switch_profile(&admin.access_token, &child).await,
            Err(NorylumError::Forbidden(_))
        ));

        // PIN correct : acces autorise.
        let admitted = auth
            .switch_profile_with_credentials(&admin.access_token, &child, None, Some(PIN))
            .await
            .unwrap();
        assert_eq!(admitted.profile_id, child);

        // PIN incorrect : verrouillage progressif.
        for _ in 0..5 {
            assert!(matches!(
                auth.switch_profile_with_credentials(
                    &admin.access_token,
                    &child,
                    None,
                    Some("000000"),
                )
                .await,
                Err(NorylumError::Forbidden(_))
            ));
        }

        // PIN correct refuse pendant le verrouillage.
        assert!(matches!(
            auth.switch_profile_with_credentials(&admin.access_token, &child, None, Some(PIN))
                .await,
            Err(NorylumError::Forbidden(_))
        ));

        // Suppression du PIN enfant par le profil principal.
        auth.delete_profile_pin(&admin.access_token, &child, PASSWORD)
            .await
            .unwrap();

        // Sans PIN apres suppression : acces direct restaure.
        let restored = auth
            .switch_profile(&admin.access_token, &child)
            .await
            .unwrap();
        assert_eq!(restored.profile_id, child);
    }

    #[tokio::test]
    async fn test_pin_migration_idempotent() {
        let db = Database::in_memory().unwrap();

        db.with_connection(|conn| {
            let mut statement = conn
                .prepare("PRAGMA table_info(profiles)")
                .map_err(storage)?;

            let columns: Vec<String> = statement
                .query_map([], |row| row.get(1))
                .map_err(storage)?
                .collect::<Result<_, _>>()
                .map_err(storage)?;

            for expected in ["pin_hash", "pin_failures", "pin_locked_until"] {
                assert!(columns.iter().any(|c| c == expected));
            }

            Ok(())
        })
        .await
        .unwrap();
    }
}
