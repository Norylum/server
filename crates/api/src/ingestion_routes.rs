//! Authorized explicit staging-ready notification.

use axum::{extract::State, http::StatusCode, Extension, Json};
use norylum_foundation::{NorylumError, RequestContext};
use norylum_library::MediaItem;
use serde::{Deserialize, Serialize};

use crate::AppState;

#[derive(Debug, Deserialize)]
pub(crate) struct ReadyRequest {
    /// Relative path under media/.norylum-staging, e.g. incoming/Movie.mkv
    source: String,
    /// Final path under media, e.g. Movies/Movie (2026).mkv
    destination: String,
}

#[derive(Serialize)]
pub(crate) struct ReadyResponse {
    media: MediaItem,
    staging_removed: bool,
}

/// POST /v1/library/ingest/ready
pub(crate) async fn notify_file_ready_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Json(payload): Json<ReadyRequest>,
) -> Result<(StatusCode, Json<ReadyResponse>), NorylumError> {
    state.authorizer.authorize_request(&ctx).await?;
    if ctx.is_child()
        || !state
            .playback
            .db()
            .is_principal_profile(ctx.account_id(), ctx.profile_id())
            .await?
    {
        return Err(NorylumError::Forbidden("Profil principal requis".into()));
    }

    // Shared with POST /v1/library/scan to prevent concurrent destructive sync.
    let _guard = state.scan_lock.lock().await;

    let prepared = state
        .ingestion
        .notify_file_ready(&payload.source, &payload.destination)
        .await?;
    let published = prepared.publish()?;
    let relative_destination = published.relative_destination().to_owned();

    let scanned = state.scanner.scan().await?;
    if !scanned
        .iter()
        .any(|item| item.relative_path == relative_destination)
    {
        return Err(NorylumError::Internal(
            "Media publie non detecte par le scanner".into(),
        ));
    }

    // Full catalog sync is transactional. The publication guard rolls back
    // the hard link if scanning or SQLite fails; staging remains intact.
    let synced = state.playback.db().sync_media_items(&scanned).await?;
    let media = synced
        .into_iter()
        .find(|item| item.relative_path == relative_destination)
        .ok_or_else(|| NorylumError::Internal("Media indexe introuvable".into()))?;
    let staging_removed = published.commit();

    Ok((
        StatusCode::CREATED,
        Json(ReadyResponse {
            media,
            staging_removed,
        }),
    ))
}
