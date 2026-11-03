//! Gestion des profils locaux Norylum.
//! Isolation par compte et administration par le profil principal.

use norylum_foundation::{AccountId, NorylumError, ProfileId, RequestContext};
use norylum_persistence::Database;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

mod management;
pub use management::UpdateProfilePayload;

fn storage(e: impl std::fmt::Display) -> NorylumError {
    NorylumError::Internal(format!("Erreur profils SQLite : {e}"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileRecord {
    pub id: ProfileId,
    pub account_id: AccountId,
    pub name: String,
    pub is_child: bool,
    pub age_rating_max: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateProfilePayload {
    pub name: String,
    pub is_child: bool,
    pub age_rating_max: Option<u32>,
}

#[derive(Clone)]
pub struct ProfileService {
    db: Database,
}

impl ProfileService {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    pub async fn list(&self, ctx: &RequestContext) -> Result<Vec<ProfileRecord>, NorylumError> {
        if !ctx.is_authenticated() {
            return Err(NorylumError::Unauthorized(
                "Authentification requise".into(),
            ));
        }

        self.db
            .with_connection(|conn| {
                let mut statement = conn
                    .prepare(
                        "SELECT id, account_id, name,
                        is_child, age_rating_max
                 FROM profiles
                 WHERE account_id = ?1
                 ORDER BY rowid ASC",
                    )
                    .map_err(storage)?;

                let rows = statement
                    .query_map(params![ctx.account_id().as_str()], |row| {
                        let child: i64 = row.get(3)?;
                        let age: Option<i64> = row.get(4)?;

                        Ok(ProfileRecord {
                            id: ProfileId::new(row.get::<_, String>(0)?),
                            account_id: AccountId::new(row.get::<_, String>(1)?),
                            name: row.get(2)?,
                            is_child: child != 0,
                            age_rating_max: age.and_then(|v| u32::try_from(v).ok()),
                        })
                    })
                    .map_err(storage)?;

                let mut profiles = Vec::new();

                for item in rows {
                    let profile = item.map_err(storage)?;
                    if !ctx.is_child() || profile.id == *ctx.profile_id() {
                        profiles.push(profile);
                    }
                }

                Ok(profiles)
            })
            .await
    }

    pub async fn create(
        &self,
        ctx: &RequestContext,
        payload: CreateProfilePayload,
    ) -> Result<ProfileRecord, NorylumError> {
        if !ctx.is_authenticated() {
            return Err(NorylumError::Unauthorized(
                "Authentification requise".into(),
            ));
        }

        if ctx.is_child() {
            return Err(NorylumError::Forbidden(
                "Un profil enfant ne peut pas creer de profil".into(),
            ));
        }

        let name = payload.name.trim().to_string();

        if name.is_empty() || name.len() > 64 {
            return Err(NorylumError::Validation(
                "Nom de profil invalide (1-64 octets)".into(),
            ));
        }

        if payload.is_child {
            match payload.age_rating_max {
                Some(age) if age <= 18 => {}
                _ => {
                    return Err(NorylumError::Validation(
                        "Profil enfant : age maximum entre 0 et 18 requis".into(),
                    ))
                }
            }
        } else if payload.age_rating_max.is_some() {
            return Err(NorylumError::Validation(
                "Un profil adulte ne doit pas avoir de limite d'age".into(),
            ));
        }

        let id = ProfileId::random();

        self.db
            .with_connection(|conn| {
                // Le premier profil du compte est le profil principal.
                // La creation est reservee a ce profil uniquement.
                let primary: Option<String> = conn
                    .query_row(
                        "SELECT id FROM profiles
                 WHERE account_id = ?1
                 ORDER BY rowid ASC LIMIT 1",
                        params![ctx.account_id().as_str()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(storage)?;

                if primary.as_deref() != Some(ctx.profile_id().as_str()) {
                    return Err(NorylumError::Forbidden("Profil principal requis".into()));
                }

                let inserted = conn
                    .execute(
                        "INSERT OR IGNORE INTO profiles
                 (id, account_id, name, is_child, age_rating_max)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![
                            id.as_str(),
                            ctx.account_id().as_str(),
                            name,
                            i64::from(payload.is_child),
                            payload.age_rating_max.map(i64::from),
                        ],
                    )
                    .map_err(storage)?;

                if inserted != 1 {
                    return Err(NorylumError::Validation(
                        "Ce nom de profil existe deja".into(),
                    ));
                }

                Ok(ProfileRecord {
                    id,
                    account_id: ctx.account_id().clone(),
                    name,
                    is_child: payload.is_child,
                    age_rating_max: payload.age_rating_max,
                })
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use norylum_auth::AuthService;

    #[tokio::test]
    async fn test_profiles_permissions() {
        let db = Database::in_memory().unwrap();
        let auth = AuthService::new(db.clone());
        let profiles = ProfileService::new(db);

        auth.bootstrap_admin("admin", "mot-de-passe-test-2026")
            .await
            .unwrap();

        let session = auth.login("admin", "mot-de-passe-test-2026").await.unwrap();

        let admin = auth.authenticate(&session.access_token).await.unwrap();

        assert_eq!(profiles.list(&admin).await.unwrap().len(), 1);

        let child = profiles
            .create(
                &admin,
                CreateProfilePayload {
                    name: "Junior".into(),
                    is_child: true,
                    age_rating_max: Some(10),
                },
            )
            .await
            .unwrap();

        assert_eq!(profiles.list(&admin).await.unwrap().len(), 2);

        let child_session = auth
            .switch_profile(&session.access_token, &child.id)
            .await
            .unwrap();

        let child_ctx = auth
            .authenticate(&child_session.access_token)
            .await
            .unwrap();

        let visible = profiles.list(&child_ctx).await.unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, child.id);

        assert!(matches!(
            profiles
                .create(
                    &child_ctx,
                    CreateProfilePayload {
                        name: "Autre".into(),
                        is_child: false,
                        age_rating_max: None,
                    },
                )
                .await,
            Err(NorylumError::Forbidden(_))
        ));

        assert!(matches!(
            profiles
                .create(
                    &admin,
                    CreateProfilePayload {
                        name: "Junior".into(),
                        is_child: true,
                        age_rating_max: Some(10),
                    },
                )
                .await,
            Err(NorylumError::Validation(_))
        ));
    }
}
