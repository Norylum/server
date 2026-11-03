//! Modification et suppression securisees des profils.

use crate::{storage, ProfileRecord, ProfileService};
use norylum_foundation::{NorylumError, ProfileId, RequestContext};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateProfilePayload {
    pub name: String,
    pub is_child: bool,
    pub age_rating_max: Option<u32>,
}

fn validate(payload: &UpdateProfilePayload) -> Result<String, NorylumError> {
    let name = payload.name.trim();

    if name.is_empty() || name.len() > 64 || name.chars().any(char::is_control) {
        return Err(NorylumError::Validation("Nom de profil invalide".into()));
    }

    if payload.is_child {
        if !matches!(payload.age_rating_max, Some(0..=18)) {
            return Err(NorylumError::Validation(
                "Un profil enfant doit avoir une limite entre 0 et 18".into(),
            ));
        }
    } else if payload.age_rating_max.is_some() {
        return Err(NorylumError::Validation(
            "Un profil adulte ne doit pas avoir de limite d'age".into(),
        ));
    }

    Ok(name.to_owned())
}

fn require_principal(tx: &Transaction<'_>, ctx: &RequestContext) -> Result<String, NorylumError> {
    if !ctx.is_authenticated() {
        return Err(NorylumError::Unauthorized(
            "Authentification requise".into(),
        ));
    }

    if ctx.is_child() {
        return Err(NorylumError::Forbidden("Profil principal requis".into()));
    }

    let principal: Option<String> = tx
        .query_row(
            "SELECT id FROM profiles
             WHERE account_id = ?1
             ORDER BY rowid ASC LIMIT 1",
            params![ctx.account_id().as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;

    match principal {
        Some(id) if id == ctx.profile_id().as_str() => Ok(id),
        _ => Err(NorylumError::Forbidden(
            "Seul le profil principal peut gerer les profils".into(),
        )),
    }
}

impl ProfileService {
    /// Remplace les informations d'un profil.
    /// Toutes ses sessions sont revoquees, notamment pour
    /// eviter l'elevation d'une ancienne session enfant.
    pub async fn update(
        &self,
        ctx: &RequestContext,
        profile_id: &ProfileId,
        payload: UpdateProfilePayload,
    ) -> Result<ProfileRecord, NorylumError> {
        if !ctx.is_authenticated() {
            return Err(NorylumError::Unauthorized(
                "Authentification requise".into(),
            ));
        }

        let name = validate(&payload)?;

        self.db
            .with_connection(|conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(storage)?;

                let principal = require_principal(&tx, ctx)?;

                let exists: Option<String> = tx
                    .query_row(
                        "SELECT id FROM profiles
                         WHERE id = ?1 AND account_id = ?2",
                        params![profile_id.as_str(), ctx.account_id().as_str()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(storage)?;

                if exists.is_none() {
                    return Err(NorylumError::NotFound("Profil introuvable".into()));
                }

                if principal == profile_id.as_str() && payload.is_child {
                    return Err(NorylumError::Forbidden(
                        "Le profil principal doit rester adulte".into(),
                    ));
                }

                let duplicate: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM profiles
                         WHERE account_id = ?1
                           AND name = ?2
                           AND id <> ?3",
                        params![
                            ctx.account_id().as_str(),
                            name.as_str(),
                            profile_id.as_str()
                        ],
                        |row| row.get(0),
                    )
                    .map_err(storage)?;

                if duplicate != 0 {
                    return Err(NorylumError::Validation(
                        "Ce nom de profil existe deja".into(),
                    ));
                }

                tx.execute(
                    "UPDATE profiles
                     SET name = ?3,
                         is_child = ?4,
                         age_rating_max = ?5,
                         pin_hash = CASE
                             WHEN ?4 != 0 THEN NULL
                             ELSE pin_hash
                         END,
                         pin_failures = 0,
                         pin_locked_until = 0
                     WHERE id = ?1 AND account_id = ?2",
                    params![
                        profile_id.as_str(),
                        ctx.account_id().as_str(),
                        name.as_str(),
                        i64::from(payload.is_child),
                        payload.age_rating_max.map(i64::from)
                    ],
                )
                .map_err(storage)?;

                tx.execute(
                    "DELETE FROM auth_sessions
                     WHERE account_id = ?1 AND profile_id = ?2",
                    params![ctx.account_id().as_str(), profile_id.as_str()],
                )
                .map_err(storage)?;

                tx.commit().map_err(storage)?;

                Ok(ProfileRecord {
                    id: profile_id.clone(),
                    account_id: ctx.account_id().clone(),
                    name,
                    is_child: payload.is_child,
                    age_rating_max: payload.age_rating_max,
                })
            })
            .await
    }

    /// Supprime un profil secondaire, ses sessions et
    /// les progressions de lecture qui lui appartiennent.
    pub async fn delete(
        &self,
        ctx: &RequestContext,
        profile_id: &ProfileId,
    ) -> Result<(), NorylumError> {
        if !ctx.is_authenticated() {
            return Err(NorylumError::Unauthorized(
                "Authentification requise".into(),
            ));
        }

        self.db
            .with_connection(|conn| {
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(storage)?;

                let principal = require_principal(&tx, ctx)?;

                if principal == profile_id.as_str() {
                    return Err(NorylumError::Forbidden(
                        "Suppression du profil principal interdite".into(),
                    ));
                }

                let exists: Option<String> = tx
                    .query_row(
                        "SELECT id FROM profiles
                         WHERE id = ?1 AND account_id = ?2",
                        params![profile_id.as_str(), ctx.account_id().as_str()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(storage)?;

                if exists.is_none() {
                    return Err(NorylumError::NotFound("Profil introuvable".into()));
                }

                tx.execute(
                    "DELETE FROM auth_sessions
                     WHERE account_id = ?1 AND profile_id = ?2",
                    params![ctx.account_id().as_str(), profile_id.as_str()],
                )
                .map_err(storage)?;

                tx.execute(
                    "DELETE FROM playback_progress
                     WHERE account_id = ?1 AND profile_id = ?2",
                    params![ctx.account_id().as_str(), profile_id.as_str()],
                )
                .map_err(storage)?;

                tx.execute(
                    "DELETE FROM profiles
                     WHERE id = ?1 AND account_id = ?2",
                    params![profile_id.as_str(), ctx.account_id().as_str()],
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
    use crate::{CreateProfilePayload, ProfileService};
    use norylum_auth::AuthService;
    use norylum_persistence::Database;

    const PASSWORD: &str = "test-password-secure-2026";

    #[tokio::test]
    async fn test_update_delete_and_session_revocation() {
        let db = Database::in_memory().unwrap();
        let auth = AuthService::new(db.clone());
        let profiles = ProfileService::new(db);

        auth.bootstrap_admin("admin", PASSWORD).await.unwrap();

        let session = auth.login("admin", PASSWORD).await.unwrap();

        let principal = auth.authenticate(&session.access_token).await.unwrap();

        let child = profiles
            .create(
                &principal,
                CreateProfilePayload {
                    name: "Junior".into(),
                    is_child: true,
                    age_rating_max: Some(10),
                },
            )
            .await
            .unwrap();

        let child_session = auth
            .switch_profile(&session.access_token, &child.id)
            .await
            .unwrap();

        let child_ctx = auth
            .authenticate(&child_session.access_token)
            .await
            .unwrap();

        assert!(matches!(
            profiles.delete(&child_ctx, &child.id).await,
            Err(NorylumError::Forbidden(_))
        ));

        assert!(matches!(
            profiles.delete(&principal, principal.profile_id()).await,
            Err(NorylumError::Forbidden(_))
        ));

        let updated = profiles
            .update(
                &principal,
                &child.id,
                UpdateProfilePayload {
                    name: "Junior 2".into(),
                    is_child: true,
                    age_rating_max: Some(16),
                },
            )
            .await
            .unwrap();

        assert_eq!(updated.age_rating_max, Some(16));

        // L'ancienne session ne doit plus fonctionner.
        assert!(auth
            .authenticate(&child_session.access_token)
            .await
            .is_err());

        let new_child_session = auth
            .switch_profile(&session.access_token, &child.id)
            .await
            .unwrap();

        // Transformation du profil enfant en adulte.
        profiles
            .update(
                &principal,
                &child.id,
                UpdateProfilePayload {
                    name: "Adulte".into(),
                    is_child: false,
                    age_rating_max: None,
                },
            )
            .await
            .unwrap();

        // Ne jamais elever les droits d'un ancien jeton.
        assert!(auth
            .authenticate(&new_child_session.access_token)
            .await
            .is_err());

        // Un adulte demande maintenant le mot de passe.
        assert!(matches!(
            auth.switch_profile(&session.access_token, &child.id).await,
            Err(NorylumError::Forbidden(_))
        ));

        let adult_session = auth
            .switch_profile_with_password(&session.access_token, &child.id, Some(PASSWORD))
            .await
            .unwrap();

        profiles.delete(&principal, &child.id).await.unwrap();

        assert!(auth
            .authenticate(&adult_session.access_token)
            .await
            .is_err());

        assert_eq!(profiles.list(&principal).await.unwrap().len(), 1);
    }
}
