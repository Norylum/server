//! Crate `playback` pour Norylum Server.
//! Gère le suivi et la persistance en mémoire de la progression de lecture vidéo
//! pour chaque profil utilisateur.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use chrono::{DateTime, Utc};
use norylum_authorization::Authorizer;
use norylum_foundation::{AccountId, DeviceId, MediaId, NorylumError, ProfileId, RequestContext};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// État de progression d'une lecture vidéo.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlaybackProgress {
    pub profile_id: ProfileId,
    pub media_id: MediaId,
    pub position_seconds: f64,
    pub duration_seconds: f64,
    pub is_completed: bool,
    pub updated_at: DateTime<Utc>,
}

/// Corps de requête pour mettre à jour la progression de lecture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateProgressPayload {
    pub media_id: MediaId,
    pub position_seconds: f64,
    pub duration_seconds: f64,
}

/// Service de suivi de progression de lecture.
#[derive(Debug, Default, Clone)]
pub struct PlaybackService {
    store: Arc<RwLock<HashMap<(ProfileId, MediaId), PlaybackProgress>>>,
}

impl PlaybackService {
    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Enregistre ou met à jour la progression de lecture pour un profil et un média donnés.
    pub async fn update_progress(
        &self,
        profile_id: ProfileId,
        media_id: MediaId,
        position_seconds: f64,
        duration_seconds: f64,
    ) -> PlaybackProgress {
        let is_completed = if duration_seconds > 0.0 {
            (position_seconds / duration_seconds) >= 0.90
        } else {
            false
        };

        let progress = PlaybackProgress {
            profile_id: profile_id.clone(),
            media_id: media_id.clone(),
            position_seconds,
            duration_seconds,
            is_completed,
            updated_at: Utc::now(),
        };

        let mut store = self.store.write().await;
        store.insert((profile_id, media_id), progress.clone());
        progress
    }

    /// Récupère la progression d'un média pour un profil donné.
    pub async fn get_progress(
        &self,
        profile_id: &ProfileId,
        media_id: &MediaId,
    ) -> Option<PlaybackProgress> {
        let store = self.store.read().await;
        store.get(&(profile_id.clone(), media_id.clone())).cloned()
    }

    /// Liste l'ensemble des progressions associées à un profil.
    pub async fn list_by_profile(&self, profile_id: &ProfileId) -> Vec<PlaybackProgress> {
        let store = self.store.read().await;
        store
            .iter()
            .filter(|((p, _), _)| p == profile_id)
            .map(|(_, prog)| prog.clone())
            .collect()
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
            ctx.profile_id().clone(),
            payload.media_id,
            payload.position_seconds,
            payload.duration_seconds,
        )
        .await;

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
        .get_progress(ctx.profile_id(), &media_id)
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
    async fn test_playback_progress_completion() {
        let service = PlaybackService::new();
        let prof = ProfileId::new("prof_1");
        let media = MediaId::new("med_film");

        // 50% lu -> non complété
        let p1 = service
            .update_progress(prof.clone(), media.clone(), 500.0, 1000.0)
            .await;
        assert!(!p1.is_completed);

        // 92% lu -> complété
        let p2 = service
            .update_progress(prof.clone(), media.clone(), 920.0, 1000.0)
            .await;
        assert!(p2.is_completed);

        let retrieved = service.get_progress(&prof, &media).await.unwrap();
        assert_eq!(retrieved.position_seconds, 920.0);
        assert!(retrieved.is_completed);
    }

    #[tokio::test]
    async fn test_playback_fail_closed_unauth() {
        let authorizer = Arc::new(DefaultAuthorizer::new());
        let playback = Arc::new(PlaybackService::new());
        let state = PlaybackState {
            playback,
            authorizer,
        };

        let headers = HeaderMap::new();
        let res = get_progress_handler(State(state), Path(MediaId::new("med_1")), headers).await;

        assert!(matches!(res, Err(NorylumError::Unauthorized(_))));
    }
}
