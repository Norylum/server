//! Crate `playback` pour Norylum Server.
//! Gère le suivi et la persistance SQLite de la progression de lecture vidéo
//! pour chaque profil utilisateur au sein d'un compte.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use norylum_authorization::Authorizer;
use norylum_foundation::{AccountId, DeviceId, MediaId, NorylumError, ProfileId, RequestContext};
use norylum_persistence::Database;
pub use norylum_persistence::PlaybackProgressRecord as PlaybackProgress;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Corps de requête pour mettre à jour la progression de lecture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateProgressPayload {
    pub media_id: MediaId,
    pub position_seconds: f64,
    pub duration_seconds: f64,
}

/// Service de suivi de progression de lecture adossé à SQLite.
#[derive(Clone)]
pub struct PlaybackService {
    db: Database,
}

impl PlaybackService {
    /// Initialise le service de playback avec une instance de base de données SQLite.
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Crée une instance avec une base SQLite en mémoire vive (pour tests).
    pub fn in_memory() -> Self {
        let db = Database::in_memory().expect("Échec d'initialisation SQLite en mémoire");
        Self { db }
    }

    /// Accède à la référence de la base de données sous-jacente.
    pub fn db(&self) -> &Database {
        &self.db
    }

    /// Enregistre ou met à jour la progression de lecture pour un compte, un profil et un média.
    /// Applique la règle de complétion automatique si position / durée >= 90%.
    pub async fn update_progress(
        &self,
        account_id: &AccountId,
        profile_id: &ProfileId,
        media_id: &MediaId,
        position_seconds: f64,
        duration_seconds: f64,
    ) -> Result<PlaybackProgress, NorylumError> {
        self.db
            .save_playback_progress(
                account_id,
                profile_id,
                media_id,
                position_seconds,
                duration_seconds,
            )
            .await
    }

    /// Récupère la progression d'un média pour un compte et un profil donnés.
    pub async fn get_progress(
        &self,
        account_id: &AccountId,
        profile_id: &ProfileId,
        media_id: &MediaId,
    ) -> Option<PlaybackProgress> {
        self.db
            .get_playback_progress(account_id, profile_id, media_id)
            .await
            .unwrap_or(None)
    }

    /// Liste l'ensemble des progressions associées à un profil.
    pub async fn list_by_profile(
        &self,
        account_id: &AccountId,
        profile_id: &ProfileId,
    ) -> Vec<PlaybackProgress> {
        self.db
            .list_playback_progress_for_profile(account_id, profile_id)
            .await
            .unwrap_or_default()
    }
}

impl Default for PlaybackService {
    fn default() -> Self {
        Self::in_memory()
    }
}

/// Dépendances pour les handlers HTTP Playback.
#[derive(Clone)]
pub struct PlaybackState {
    pub playback: Arc<PlaybackService>,
    pub authorizer: Arc<dyn Authorizer>,
}

fn extract_context(headers: &HeaderMap) -> RequestContext {
    let account_id = headers
        .get("x-account-id")
        .and_then(|v| v.to_str().ok())
        .map(AccountId::new);

    let profile_id = headers
        .get("x-profile-id")
        .and_then(|v| v.to_str().ok())
        .map(ProfileId::new);

    let device_id = headers
        .get("x-device-id")
        .and_then(|v| v.to_str().ok())
        .map(DeviceId::new)
        .unwrap_or_else(|| DeviceId::new("device-player"));

    let is_child = headers
        .get("x-is-child")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    match (account_id, profile_id) {
        (Some(acc), Some(prof)) => {
            RequestContext::authenticated(acc, prof, device_id, is_child, None)
        }
        _ => RequestContext::unauthenticated(),
    }
}

/// Handler HTTP `POST /v1/playback/progress`
pub async fn update_progress_handler(
    State(state): State<PlaybackState>,
    headers: HeaderMap,
    Json(payload): Json<UpdateProgressPayload>,
) -> Result<Json<PlaybackProgress>, NorylumError> {
    let ctx = extract_context(&headers);
    state.authorizer.authorize_request(&ctx).await?;

    let progress = state
        .playback
        .update_progress(
            ctx.account_id(),
            ctx.profile_id(),
            &payload.media_id,
            payload.position_seconds,
            payload.duration_seconds,
        )
        .await?;

    Ok(Json(progress))
}

/// Handler HTTP `GET /v1/playback/:media_id`
pub async fn get_progress_handler(
    State(state): State<PlaybackState>,
    Path(media_id): Path<MediaId>,
    headers: HeaderMap,
) -> Result<Json<PlaybackProgress>, NorylumError> {
    let ctx = extract_context(&headers);
    state.authorizer.authorize_request(&ctx).await?;

    let progress = state
        .playback
        .get_progress(ctx.account_id(), ctx.profile_id(), &media_id)
        .await
        .ok_or_else(|| {
            NorylumError::NotFound(format!(
                "Aucune progression trouvée pour le média '{}' et le profil '{}'",
                media_id,
                ctx.profile_id()
            ))
        })?;

    Ok(Json(progress))
}

#[cfg(test)]
mod tests {
    use super::*;
    use norylum_authorization::DefaultAuthorizer;

    #[tokio::test]
    async fn test_playback_progress_completion_and_sqlite() {
        let service = PlaybackService::in_memory();
        let acc = AccountId::new("acc_test");
        let prof = ProfileId::new("prof_1");
        let media = MediaId::new("med_film");

        // 50% lu -> non complété
        let p1 = service
            .update_progress(&acc, &prof, &media, 500.0, 1000.0)
            .await
            .unwrap();
        assert!(!p1.is_completed);

        // 92% lu -> complété
        let p2 = service
            .update_progress(&acc, &prof, &media, 920.0, 1000.0)
            .await
            .unwrap();
        assert!(p2.is_completed);

        let retrieved = service.get_progress(&acc, &prof, &media).await.unwrap();
        assert_eq!(retrieved.position_seconds, 920.0);
        assert!(retrieved.is_completed);
        assert_eq!(retrieved.account_id, acc);
        assert_eq!(retrieved.profile_id, prof);
    }

    #[tokio::test]
    async fn test_playback_fail_closed_unauth() {
        let authorizer = Arc::new(DefaultAuthorizer::new());
        let playback = Arc::new(PlaybackService::in_memory());
        let state = PlaybackState {
            playback,
            authorizer,
        };

        let headers = HeaderMap::new();
        let res = get_progress_handler(State(state), Path(MediaId::new("med_1")), headers).await;

        assert!(matches!(res, Err(NorylumError::Unauthorized(_))));
    }
}
