//! API de reprise et synchronisation de lecture.
//! Les contextes viennent du middleware d'authentification.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension, Json,
};
use norylum_authorization::{Authorizer, MediaAccessInfo};
use norylum_catalog::CatalogService;
use norylum_foundation::{MediaId, NorylumError, RequestContext};
use norylum_persistence::{PlaybackProgressRecord, SyncWriteOperation};
use norylum_playback::{PlaybackService, UpdateProgressPayload};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct PlaybackSyncState {
    pub playback: Arc<PlaybackService>,
    pub catalog: Arc<CatalogService>,
    pub authorizer: Arc<dyn Authorizer>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SyncProgressPayload {
    position_seconds: f64,
    duration_seconds: f64,
    #[serde(default)]
    expected_revision: Option<u64>,
    #[serde(default)]
    operation_id: Option<String>,
}

async fn authorize_media(
    state: &PlaybackSyncState,
    ctx: &RequestContext,
    media_id: &MediaId,
) -> Result<(), NorylumError> {
    state.authorizer.authorize_request(ctx).await?;

    let media = state
        .catalog
        .get_by_id(media_id)
        .await
        .ok_or_else(|| NorylumError::NotFound("Media introuvable".into()))?;

    state
        .authorizer
        .authorize_media_access(
            ctx,
            &MediaAccessInfo::new(media.id, media.is_child_safe, media.age_rating),
        )
        .await
}

async fn visible_progress(
    state: &PlaybackSyncState,
    ctx: &RequestContext,
) -> Result<Vec<PlaybackProgressRecord>, NorylumError> {
    state.authorizer.authorize_request(ctx).await?;

    let entries = state
        .playback
        .db()
        .list_playback_progress_for_profile(ctx.account_id(), ctx.profile_id())
        .await?;

    let mut visible = Vec::new();

    for entry in entries {
        match authorize_media(state, ctx, &entry.media_id).await {
            Ok(()) => visible.push(entry),
            Err(NorylumError::Forbidden(_)) | Err(NorylumError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
    }

    visible.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.media_id.as_str().cmp(b.media_id.as_str()))
    });

    Ok(visible)
}

/// GET /v1/playback/sync
pub(crate) async fn list_sync_handler(
    State(state): State<PlaybackSyncState>,
    Extension(ctx): Extension<RequestContext>,
) -> Result<Json<Vec<PlaybackProgressRecord>>, NorylumError> {
    Ok(Json(visible_progress(&state, &ctx).await?))
}

/// GET /v1/playback/continue-watching
pub(crate) async fn continue_watching_handler(
    State(state): State<PlaybackSyncState>,
    Extension(ctx): Extension<RequestContext>,
) -> Result<Json<Vec<PlaybackProgressRecord>>, NorylumError> {
    let mut entries = visible_progress(&state, &ctx).await?;

    entries.retain(|entry| !entry.is_completed && entry.position_seconds > 0.0);

    entries.truncate(50);

    Ok(Json(entries))
}

/// GET /v1/playback/history
///
/// Historique recent : un etat par media, pas chaque
/// evenement individuel de lecture.
pub(crate) async fn history_handler(
    State(state): State<PlaybackSyncState>,
    Extension(ctx): Extension<RequestContext>,
) -> Result<Json<Vec<PlaybackProgressRecord>>, NorylumError> {
    let mut entries = visible_progress(&state, &ctx).await?;

    entries.retain(|entry| entry.position_seconds > 0.0);
    entries.truncate(100);

    Ok(Json(entries))
}

/// PUT /v1/playback/sync/:media_id
///
/// operation_id facultatif pour les anciens clients.
/// Lors d'un conflit, le client recoit l'etat actuel.
pub(crate) async fn sync_progress_handler(
    State(state): State<PlaybackSyncState>,
    Extension(ctx): Extension<RequestContext>,
    Path(media_id): Path<MediaId>,
    Json(payload): Json<SyncProgressPayload>,
) -> Result<Response, NorylumError> {
    authorize_media(&state, &ctx, &media_id).await?;

    let db = state.playback.db();

    let update = if let Some(id) = payload.operation_id.as_deref() {
        db.sync_playback_progress_idempotent(
            ctx.account_id(),
            ctx.profile_id(),
            &media_id,
            SyncWriteOperation {
                operation_id: id,
                position_seconds: payload.position_seconds,
                duration_seconds: payload.duration_seconds,
                expected_revision: payload.expected_revision,
            },
        )
        .await
    } else {
        // Compatibilite avec les clients Playback Sync V1.
        db.sync_playback_progress(
            ctx.account_id(),
            ctx.profile_id(),
            &media_id,
            payload.position_seconds,
            payload.duration_seconds,
            payload.expected_revision,
        )
        .await
    };

    match update {
        Ok(progress) => Ok(Json(progress).into_response()),
        Err(NorylumError::Conflict(message)) => {
            let current = db
                .get_playback_progress(ctx.account_id(), ctx.profile_id(), &media_id)
                .await?;

            Ok((
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "code": "CONFLICT",
                    "error": message,
                    "current": current,
                })),
            )
                .into_response())
        }
        Err(other) => Err(other),
    }
}

/// Ancien endpoint, maintenu pour les clients existants.
/// Il incremente la revision mais reste last-write-wins.
pub(crate) async fn legacy_update_handler(
    State(state): State<PlaybackSyncState>,
    Extension(ctx): Extension<RequestContext>,
    Json(payload): Json<UpdateProgressPayload>,
) -> Result<Json<PlaybackProgressRecord>, NorylumError> {
    authorize_media(&state, &ctx, &payload.media_id).await?;

    Ok(Json(
        state
            .playback
            .update_progress(
                ctx.account_id(),
                ctx.profile_id(),
                &payload.media_id,
                payload.position_seconds,
                payload.duration_seconds,
            )
            .await?,
    ))
}

/// GET /v1/playback/:media_id
pub(crate) async fn legacy_get_handler(
    State(state): State<PlaybackSyncState>,
    Extension(ctx): Extension<RequestContext>,
    Path(media_id): Path<MediaId>,
) -> Result<Json<PlaybackProgressRecord>, NorylumError> {
    authorize_media(&state, &ctx, &media_id).await?;

    let record = state
        .playback
        .db()
        .get_playback_progress(ctx.account_id(), ctx.profile_id(), &media_id)
        .await?
        .ok_or_else(|| NorylumError::NotFound("Aucune progression pour ce media".into()))?;

    Ok(Json(record))
}
