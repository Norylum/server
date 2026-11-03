//! Authenticated, principal-only torrent control.
//! The API records intent; the separate seeder owns network activity.

use crate::AppState;
use axum::{
    extract::{Path, State},
    Extension, Json,
};
use norylum_foundation::{MediaId, NorylumError, RequestContext};
use norylum_persistence::{TorrentSettings, TorrentShare};
use serde::Deserialize;
use serde_json::{json, Value};

async fn principal(state: &AppState, ctx: &RequestContext) -> Result<(), NorylumError> {
    state.authorizer.authorize_request(ctx).await?;

    if ctx.is_child()
        || !state
            .playback
            .db()
            .is_principal_profile(ctx.account_id(), ctx.profile_id())
            .await?
    {
        return Err(NorylumError::Forbidden("Profil principal requis".into()));
    }

    Ok(())
}

#[derive(Deserialize)]
pub(crate) struct EnableRequest {
    confirm_distribution_rights: bool,
}

#[derive(Deserialize)]
pub(crate) struct LimitRequest {
    upload_kibps: u32,
}

pub(crate) async fn list_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
) -> Result<Json<Vec<TorrentShare>>, NorylumError> {
    principal(&state, &ctx).await?;
    Ok(Json(state.playback.db().torrent_shares().await?))
}

pub(crate) async fn get_settings_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
) -> Result<Json<TorrentSettings>, NorylumError> {
    principal(&state, &ctx).await?;
    Ok(Json(state.playback.db().torrent_settings().await?))
}

pub(crate) async fn set_settings_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Json(payload): Json<LimitRequest>,
) -> Result<Json<TorrentSettings>, NorylumError> {
    principal(&state, &ctx).await?;

    Ok(Json(
        state
            .playback
            .db()
            .set_torrent_limit(payload.upload_kibps)
            .await?,
    ))
}

pub(crate) async fn enable_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(media_id): Path<MediaId>,
    Json(payload): Json<EnableRequest>,
) -> Result<Json<Value>, NorylumError> {
    principal(&state, &ctx).await?;

    if !payload.confirm_distribution_rights {
        return Err(NorylumError::Validation(
            "Confirmation explicite des droits de distribution requise".into(),
        ));
    }

    let db = state.playback.db();

    let item = db
        .get_media_by_id(&media_id)
        .await?
        .ok_or_else(|| NorylumError::NotFound("Media Norylum non indexe".into()))?;

    if db.embedded_tags_for_media(&media_id).await?.is_none() {
        return Err(NorylumError::Validation(
            "Association TMDB requise avant le partage".into(),
        ));
    }

    // Validate the database path against the real library volume.
    let path = norylum_library::PathSanitizer::sanitize_path(&state.library_root, &item.file_path)?;

    if !path.is_file() {
        return Err(NorylumError::Validation(
            "Fichier media non disponible".into(),
        ));
    }

    db.set_torrent_enabled(&media_id, true).await?;

    Ok(Json(json!({
        "media_id": media_id,
        "status": "queued",
        "message": "Partage autorise; attente du seeder"
    })))
}

pub(crate) async fn disable_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(media_id): Path<MediaId>,
) -> Result<Json<Value>, NorylumError> {
    principal(&state, &ctx).await?;

    state
        .playback
        .db()
        .set_torrent_enabled(&media_id, false)
        .await?;

    Ok(Json(json!({
        "media_id": media_id,
        "status": "stopping",
        "message": "Arret demande; confirmation apres synchronisation"
    })))
}
