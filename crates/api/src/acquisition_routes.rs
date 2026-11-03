//! Authenticated acquisition intent. No downloader is started.

use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use norylum_foundation::{NorylumError, RequestContext};
use norylum_metadata::TmdbKind;
use norylum_persistence::AcquisitionRequest;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RequestPayload {
    kind: TmdbKind,
    tmdb_id: u64,
    season: Option<u32>,
    episode: Option<u32>,
    quality_profile: Option<String>,
    language: Option<String>,
}

async fn principal(state: &AppState, ctx: &RequestContext) -> Result<(), NorylumError> {
    state.authorizer.authorize_request(ctx).await?;

    if ctx.is_child()
        || !state
            .playback
            .db()
            .is_principal_profile(ctx.account_id(), ctx.profile_id())
            .await?
    {
        return Err(NorylumError::Forbidden(
            "Profil principal requis pour Acquisition V1".into(),
        ));
    }

    Ok(())
}

fn valid_request_id(id: &str) -> bool {
    id.strip_prefix("acq_").is_some_and(|suffix| {
        suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

/// POST /v1/acquisition/requests
pub(crate) async fn create_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Json(payload): Json<RequestPayload>,
) -> Result<(StatusCode, Json<AcquisitionRequest>), NorylumError> {
    principal(&state, &ctx).await?;

    let quality = payload.quality_profile.as_deref().unwrap_or("1080p");

    let language = payload.language.as_deref().unwrap_or("fr");

    let request = state
        .playback
        .db()
        .request_acquisition(
            ctx.account_id(),
            ctx.profile_id(),
            payload.kind.as_str(),
            payload.tmdb_id,
            payload.season,
            payload.episode,
            quality,
            language,
        )
        .await?;

    Ok((StatusCode::ACCEPTED, Json(request)))
}

/// GET /v1/acquisition/requests
pub(crate) async fn list_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
) -> Result<Json<Vec<AcquisitionRequest>>, NorylumError> {
    principal(&state, &ctx).await?;

    Ok(Json(
        state
            .playback
            .db()
            .list_acquisition_requests(ctx.account_id())
            .await?,
    ))
}

/// POST /v1/acquisition/requests/:id/approve
pub(crate) async fn approve_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<AcquisitionRequest>, NorylumError> {
    principal(&state, &ctx).await?;

    if !valid_request_id(&id) {
        return Err(NorylumError::NotFound(
            "Demande d'acquisition introuvable".into(),
        ));
    }

    let request = state
        .playback
        .db()
        .approve_acquisition_request(ctx.account_id(), &id)
        .await?;

    Ok(Json(request))
}

/// POST /v1/acquisition/requests/:id/cancel
pub(crate) async fn cancel_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<AcquisitionRequest>, NorylumError> {
    principal(&state, &ctx).await?;

    if !valid_request_id(&id) {
        return Err(NorylumError::NotFound(
            "Demande d'acquisition introuvable".into(),
        ));
    }

    let request = state
        .playback
        .db()
        .cancel_acquisition_request(ctx.account_id(), &id)
        .await?;

    Ok(Json(request))
}

/// GET /v1/acquisition/requests/:id
pub(crate) async fn get_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<AcquisitionRequest>, NorylumError> {
    principal(&state, &ctx).await?;

    if !valid_request_id(&id) {
        return Err(NorylumError::NotFound(
            "Demande d'acquisition introuvable".into(),
        ));
    }

    let request = state
        .playback
        .db()
        .get_acquisition_request(ctx.account_id(), &id)
        .await?
        .ok_or_else(|| NorylumError::NotFound("Demande d'acquisition introuvable".into()))?;

    Ok(Json(request))
}
