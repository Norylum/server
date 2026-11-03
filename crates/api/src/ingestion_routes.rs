//! Principal-profile ingestion, shared by the HTTP endpoint and the internal marker worker.
use crate::AppState;
use axum::{extract::State, http::StatusCode, Extension, Json};
use norylum_foundation::{NorylumError, RequestContext};
use norylum_library::MediaItem;
use norylum_persistence::IngestionJob;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub(crate) struct ReadyRequest {
    source: String,
    destination: String,
}

#[derive(Serialize)]
pub(crate) struct ReadyResponse {
    pub media: MediaItem,
    pub staging_removed: bool,
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
        return Err(NorylumError::Forbidden("Profil principal requis".into()));
    }
    Ok(())
}

/// Shared serialisation with /scan: only one destructive filesystem operation at a time.
pub(crate) async fn process_ready(
    state: &AppState,
    source: &str,
    destination: &str,
) -> Result<ReadyResponse, NorylumError> {
    let _guard = state.scan_lock.lock().await;
    let db = state.playback.db();
    db.begin_ingestion(source, destination).await?;
    let result = async {
        let prepared = state
            .ingestion
            .notify_file_ready(source, destination)
            .await?;
        let published = prepared.publish()?;
        let item = state
            .scanner
            .scan_one(published.relative_destination())
            .await?;
        // Transactionally index only this file, without purging existing media.
        let media = db.upsert_ingested_media(&item).await?;
        let staging_removed = published.commit();
        Ok::<ReadyResponse, NorylumError>(ReadyResponse {
            media,
            staging_removed,
        })
    }
    .await;
    // Preserve original processing error even if journaling fails.
    match &result {
        Ok(response) => {
            if let Err(error) = db.finish_ingestion(source, Ok(&response.media.id)).await {
                tracing::error!(error = %error, "Media publie mais journal non actualise");
            }
        }
        Err(error) => {
            if let Err(journal_error) = db.finish_ingestion(source, Err(error)).await {
                tracing::error!(error = %journal_error, "Echec du journal d'ingestion");
            }
        }
    }
    result
}

/// POST /v1/library/ingest/ready
pub(crate) async fn notify_file_ready_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Json(payload): Json<ReadyRequest>,
) -> Result<(StatusCode, Json<ReadyResponse>), NorylumError> {
    principal(&state, &ctx).await?;
    let result = process_ready(&state, &payload.source, &payload.destination).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

/// GET /v1/library/ingest/jobs (principal profile only)
pub(crate) async fn list_jobs_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
) -> Result<Json<Vec<IngestionJob>>, NorylumError> {
    principal(&state, &ctx).await?;
    Ok(Json(state.playback.db().list_ingestion_jobs().await?))
}
