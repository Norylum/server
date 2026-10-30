//! Crate `api` pour Norylum Server.
//! Assemble les routes HTTP Axum, gère l'état partagé `AppState` et unifie
//! le catalogue, la bibliothèque, les films, les séries/saisons/épisodes, le streaming et la progression.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::{Json, Router};
use norylum_authorization::Authorizer;
use norylum_catalog::{CatalogService, SeasonDetails, ShowDetails, ShowRecord};
use norylum_foundation::{NorylumError, ShowId};
use norylum_library::{MediaItem, MediaScanner};
use norylum_playback::{PlaybackService, PlaybackState};
use norylum_streaming::StreamingState;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;

/// État partagé unifié du monolithe modulaire Norylum.
#[derive(Clone)]
pub struct AppState {
    pub library_root: PathBuf,
    pub catalog: Arc<CatalogService>,
    pub authorizer: Arc<dyn Authorizer>,
    pub playback: Arc<PlaybackService>,
    pub scanner: Arc<MediaScanner>,
}

impl AppState {
    pub fn new(
        library_root: PathBuf,
        catalog: Arc<CatalogService>,
        authorizer: Arc<dyn Authorizer>,
        playback: Arc<PlaybackService>,
    ) -> Self {
        let scanner = Arc::new(MediaScanner::new(library_root.clone()));
        Self {
            library_root,
            catalog,
            authorizer,
            playback,
            scanner,
        }
    }
}

#[derive(Serialize)]
pub struct ScanResponse {
    pub scanned_count: usize,
    pub message: String,
}

/// Handler `POST /v1/library/scan`
pub async fn scan_library_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ScanResponse>, NorylumError> {
    let ctx = norylum_streaming::extract_request_context_from_headers(&headers);
    state.authorizer.authorize_request(&ctx).await?;

    tracing::info!(
        account_id = %ctx.account_id(),
        profile_id = %ctx.profile_id(),
        "Déclenchement du scan de la bibliothèque média"
    );

    let items = state.scanner.scan().await?;
    let count = items.len();
    state.catalog.replace_all(items).await;

    Ok(Json(ScanResponse {
        scanned_count: count,
        message: format!("Scan terminé avec succès. {} éléments indexés.", count),
    }))
}

/// Handler `GET /v1/library/media`
pub async fn list_library_media_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<MediaItem>>, NorylumError> {
    let ctx = norylum_streaming::extract_request_context_from_headers(&headers);
    state.authorizer.authorize_request(&ctx).await?;

    let media_list = state
        .catalog
        .list_for_context(&ctx, state.authorizer.as_ref())
        .await;

    Ok(Json(media_list))
}

/// Handler `GET /v1/library/movies`
pub async fn list_library_movies_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<MediaItem>>, NorylumError> {
    let ctx = norylum_streaming::extract_request_context_from_headers(&headers);
    state.authorizer.authorize_request(&ctx).await?;

    let movies = state
        .catalog
        .list_movies_for_context(&ctx, state.authorizer.as_ref())
        .await;

    Ok(Json(movies))
}

/// Handler `GET /v1/library/shows`
pub async fn list_library_shows_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ShowRecord>>, NorylumError> {
    let ctx = norylum_streaming::extract_request_context_from_headers(&headers);
    state.authorizer.authorize_request(&ctx).await?;

    let shows = state
        .catalog
        .list_shows_for_context(&ctx, state.authorizer.as_ref())
        .await;

    Ok(Json(shows))
}

/// Handler `GET /v1/library/shows/:show_id`
pub async fn get_library_show_handler(
    State(state): State<AppState>,
    Path(show_id): Path<ShowId>,
    headers: HeaderMap,
) -> Result<Json<ShowDetails>, NorylumError> {
    let ctx = norylum_streaming::extract_request_context_from_headers(&headers);
    state.authorizer.authorize_request(&ctx).await?;

    let show_details = state
        .catalog
        .get_show_details_for_context(&show_id, &ctx, state.authorizer.as_ref())
        .await?
        .ok_or_else(|| NorylumError::NotFound(format!("Série '{}' introuvable", show_id)))?;

    Ok(Json(show_details))
}

/// Handler `GET /v1/library/shows/:show_id/seasons/:season_number`
pub async fn get_library_season_handler(
    State(state): State<AppState>,
    Path((show_id, season_number)): Path<(ShowId, u32)>,
    headers: HeaderMap,
) -> Result<Json<SeasonDetails>, NorylumError> {
    let ctx = norylum_streaming::extract_request_context_from_headers(&headers);
    state.authorizer.authorize_request(&ctx).await?;

    let season_details = state
        .catalog
        .get_season_details_for_context(&show_id, season_number, &ctx, state.authorizer.as_ref())
        .await?
        .ok_or_else(|| {
            NorylumError::NotFound(format!(
                "Saison {} introuvable pour la série '{}'",
                season_number, show_id
            ))
        })?;

    Ok(Json(season_details))
}

/// Construit et configure le routeur Axum pour Norylum Server.
pub fn create_router(state: AppState) -> Router {
    let streaming_state = StreamingState {
        library_root: state.library_root.clone(),
        catalog: state.catalog.clone(),
        authorizer: state.authorizer.clone(),
    };

    let playback_state = PlaybackState {
        playback: state.playback.clone(),
        authorizer: state.authorizer.clone(),
    };

    let library_routes = Router::new()
        .route("/scan", axum::routing::post(scan_library_handler))
        .route("/media", axum::routing::get(list_library_media_handler))
        .route("/movies", axum::routing::get(list_library_movies_handler))
        .route("/shows", axum::routing::get(list_library_shows_handler))
        .route(
            "/shows/:show_id",
            axum::routing::get(get_library_show_handler),
        )
        .route(
            "/shows/:show_id/seasons/:season_number",
            axum::routing::get(get_library_season_handler),
        )
        .with_state(state);

    let streaming_routes = Router::new()
        .route(
            "/:media_id",
            axum::routing::get(norylum_streaming::stream_media_handler),
        )
        .with_state(streaming_state);

    let playback_routes = Router::new()
        .route(
            "/progress",
            axum::routing::post(norylum_playback::update_progress_handler),
        )
        .route(
            "/:media_id",
            axum::routing::get(norylum_playback::get_progress_handler),
        )
        .with_state(playback_state);

    Router::new()
        .route("/health", axum::routing::get(|| async { "OK" }))
        .nest("/v1/library", library_routes)
        .nest("/v1/media", streaming_routes)
        .nest("/v1/playback", playback_routes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use norylum_authorization::DefaultAuthorizer;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_health_check_endpoint() {
        let temp_dir = std::env::temp_dir();
        let state = AppState::new(
            temp_dir,
            Arc::new(CatalogService::in_memory()),
            Arc::new(DefaultAuthorizer::new()),
            Arc::new(PlaybackService::in_memory()),
        );

        let app = create_router(state);

        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_movies_and_shows_fail_closed_unauth() {
        let temp_dir = std::env::temp_dir();
        let state = AppState::new(
            temp_dir,
            Arc::new(CatalogService::in_memory()),
            Arc::new(DefaultAuthorizer::new()),
            Arc::new(PlaybackService::in_memory()),
        );

        let app = create_router(state);

        // 1. GET /v1/library/movies sans en-tête -> 401
        let req_movies = Request::builder()
            .uri("/v1/library/movies")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req_movies).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // 2. GET /v1/library/shows sans en-tête -> 401
        let req_shows = Request::builder()
            .uri("/v1/library/shows")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req_shows).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
