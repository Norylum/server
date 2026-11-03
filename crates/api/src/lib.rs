//! Crate `api` pour Norylum Server.
//! Assemble les routes HTTP Axum, gère l'état partagé `AppState` et unifie
//! le catalogue, la bibliothèque, les films, les séries/saisons/épisodes, le streaming et la progression.

mod auth_routes;
mod device_routes;
mod ingestion_routes;
mod ingestion_worker;
pub use ingestion_worker::run_ingestion_worker;
mod metadata_routes;
mod playback_sync_routes;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::middleware;
use axum::{Json, Router};
use norylum_auth::AuthService;
use norylum_authorization::Authorizer;
use norylum_catalog::{CatalogService, SeasonDetails, ShowDetails, ShowRecord};
use norylum_devices::DeviceService;
use norylum_foundation::{NorylumError, ShowId};
use norylum_library::{IngestionService, MediaItem, MediaScanner};
use norylum_metadata::TmdbClient;
use norylum_playback::PlaybackService;
use norylum_profiles::ProfileService;
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
    pub ingestion: Arc<IngestionService>,
    pub scan_lock: Arc<tokio::sync::Mutex<()>>,
    pub auth: Arc<AuthService>,
    pub devices: Arc<DeviceService>,
    pub profiles: Arc<ProfileService>,
}

impl AppState {
    pub fn new(
        library_root: PathBuf,
        catalog: Arc<CatalogService>,
        authorizer: Arc<dyn Authorizer>,
        playback: Arc<PlaybackService>,
    ) -> Self {
        let scanner = Arc::new(MediaScanner::new(library_root.clone()));
        let ingestion = Arc::new(IngestionService::new(library_root.clone()));
        let auth = Arc::new(AuthService::new(playback.db().clone()));
        let devices = Arc::new(DeviceService::new(playback.db().clone()));
        let profiles = Arc::new(ProfileService::new(playback.db().clone()));
        Self {
            library_root,
            catalog,
            authorizer,
            playback,
            scanner,
            ingestion,
            scan_lock: Arc::new(tokio::sync::Mutex::new(())),
            auth,
            devices,
            profiles,
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

    let _guard = state.scan_lock.lock().await;
    let items = state.scanner.scan().await?;
    let count = items.len();
    // Propagate SQLite failures rather than returning a false positive.
    state.playback.db().sync_media_items(&items).await?;

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
    let auth = state.auth.clone();
    let devices = state.devices.clone();
    let profiles = state.profiles.clone();
    let streaming_state = StreamingState {
        library_root: state.library_root.clone(),
        catalog: state.catalog.clone(),
        authorizer: state.authorizer.clone(),
    };

    let playback_sync_state = playback_sync_routes::PlaybackSyncState {
        playback: state.playback.clone(),
        catalog: state.catalog.clone(),
        authorizer: state.authorizer.clone(),
    };

    let metadata_state = metadata_routes::MetadataState {
        catalog: state.catalog.clone(),
        authorizer: state.authorizer.clone(),
        db: state.playback.db().clone(),
        tmdb: TmdbClient::from_env(),
    };

    let metadata_routes = Router::new()
        .route(
            "/search",
            axum::routing::get(metadata_routes::search_handler),
        )
        .route(
            "/media/:media_id/suggestions",
            axum::routing::get(metadata_routes::suggest_movie_handler),
        )
        .route(
            "/media/:media_id",
            axum::routing::get(metadata_routes::get_movie_handler)
                .put(metadata_routes::set_movie_handler),
        )
        .route(
            "/shows/:show_id/suggestions",
            axum::routing::get(metadata_routes::suggest_show_handler),
        )
        .route(
            "/shows/:show_id",
            axum::routing::get(metadata_routes::get_show_handler)
                .put(metadata_routes::set_show_handler),
        )
        .with_state(metadata_state);

    let library_routes = Router::new()
        .route("/scan", axum::routing::post(scan_library_handler))
        .route(
            "/ingest/ready",
            axum::routing::post(ingestion_routes::notify_file_ready_handler),
        )
        .route(
            "/ingest/jobs",
            axum::routing::get(ingestion_routes::list_jobs_handler),
        )
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
            axum::routing::post(playback_sync_routes::legacy_update_handler),
        )
        .route(
            "/sync",
            axum::routing::get(playback_sync_routes::list_sync_handler),
        )
        .route(
            "/sync/:media_id",
            axum::routing::put(playback_sync_routes::sync_progress_handler),
        )
        .route(
            "/continue-watching",
            axum::routing::get(playback_sync_routes::continue_watching_handler),
        )
        .route(
            "/history",
            axum::routing::get(playback_sync_routes::history_handler),
        )
        .route(
            "/:media_id",
            axum::routing::get(playback_sync_routes::legacy_get_handler),
        )
        .with_state(playback_sync_state);

    let protected = Router::new()
        .nest("/v1/library", library_routes)
        .nest("/v1/media", streaming_routes)
        .nest("/v1/playback", playback_routes)
        .nest("/v1/metadata", metadata_routes)
        .route(
            "/v1/profiles",
            axum::routing::get(auth_routes::list_profiles_handler)
                .post(auth_routes::create_profile_handler)
                .with_state(profiles.clone()),
        )
        .route(
            "/v1/profiles/:profile_id",
            axum::routing::put(auth_routes::update_profile_handler)
                .delete(auth_routes::delete_profile_handler)
                .with_state(profiles),
        )
        .route(
            "/v1/profiles/:profile_id/pin",
            axum::routing::put(auth_routes::set_profile_pin_handler)
                .delete(auth_routes::delete_profile_pin_handler)
                .with_state(auth.clone()),
        )
        .route(
            "/v1/devices",
            axum::routing::get(device_routes::list_devices_handler).with_state(devices.clone()),
        )
        .route(
            "/v1/devices/sessions",
            axum::routing::get(device_routes::list_sessions_handler).with_state(devices.clone()),
        )
        .route(
            "/v1/devices/sessions/:session_id",
            axum::routing::delete(device_routes::revoke_session_handler)
                .with_state(devices.clone()),
        )
        .route(
            "/v1/devices/maintenance",
            axum::routing::post(device_routes::maintenance_handler).with_state(devices.clone()),
        )
        .route(
            "/v1/devices/:device_id/history",
            axum::routing::delete(device_routes::forget_device_handler).with_state(devices.clone()),
        )
        .route(
            "/v1/devices/:device_id",
            axum::routing::delete(device_routes::revoke_device_handler)
                .patch(device_routes::rename_device_handler)
                .with_state(devices),
        )
        .route("/v1/auth/me", axum::routing::get(auth_routes::me_handler))
        .route(
            "/v1/auth/logout",
            axum::routing::post(auth_routes::logout_handler).with_state(auth.clone()),
        )
        .route(
            "/v1/auth/switch-profile",
            axum::routing::post(auth_routes::switch_profile_handler).with_state(auth.clone()),
        )
        .layer(middleware::from_fn_with_state(
            auth.clone(),
            auth_routes::auth_middleware,
        ));

    Router::new()
        .route("/health", axum::routing::get(|| async { "OK" }))
        .route(
            "/v1/auth/login",
            axum::routing::post(auth_routes::login_handler).with_state(auth),
        )
        .merge(protected)
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
