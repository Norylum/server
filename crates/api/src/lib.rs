//! Crate `api` pour Norylum Server.
//! Assemble les routes HTTP Axum, gère l'état partagé `AppState` et unifie
//! le catalogue, la bibliothèque, les films, les séries/saisons/épisodes, le streaming et la progression.

mod acquisition_routes;
mod auth_routes;
mod device_routes;
mod ingestion_routes;
mod ingestion_worker;
pub use ingestion_worker::run_ingestion_worker;
mod metadata_routes;
mod playback_sync_routes;
mod torrent_routes;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::middleware;
use axum::response::Response;
use axum::{Json, Router};
use norylum_auth::rate_limit::{auth_rate_limit, new_auth_rate_limiter};
use norylum_auth::AuthService;
use norylum_authorization::Authorizer;
use norylum_catalog::{CatalogService, SeasonDetails, ShowDetails, ShowRecord};
use norylum_devices::DeviceService;
use norylum_foundation::{NorylumError, ShowId};
use norylum_library::{IngestionService, MediaItem, MediaScanner};
use norylum_metadata::{PosterCache, TmdbClient};
use norylum_playback::PlaybackService;
use norylum_profiles::ProfileService;
use axum::extract::Query;
use norylum_streaming::StreamingState;
use norylum_transcoding::{HwAccel, TranscodeRequest};
use tokio_util::io::ReaderStream;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;

/// GET /v1/images/posters/:filename — sert une affiche mise en cache localement.
/// Route publique : pas de jeton requis (les navigateurs ne peuvent pas envoyer
/// des en-têtes Authorization sur une balise <img>).
pub async fn serve_poster_handler(
    State(poster_cache): State<Arc<PosterCache>>,
    Path(filename): Path<String>,
) -> Result<Response<Body>, NorylumError> {
    let path = poster_cache
        .cached_path(&filename)
        .ok_or_else(|| NorylumError::NotFound("Affiche introuvable".into()))?;

    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| NorylumError::NotFound("Affiche introuvable".into()))?;

    let content_type = if filename.to_ascii_lowercase().ends_with(".png") {
        "image/png"
    } else {
        "image/jpeg"
    };

    let mut resp = Response::new(Body::from(bytes));
    resp.headers_mut().insert(
        "Content-Type",
        HeaderValue::from_static(content_type),
    );
    resp.headers_mut().insert(
        "Cache-Control",
        HeaderValue::from_static("max-age=31536000, immutable"),
    );

    Ok(resp)
}

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
    /// Accélérateur GPU détecté au démarrage. Partagé avec le handler de transcodage.
    pub hw_accel: HwAccel,
}

impl AppState {
    pub fn new(
        library_root: PathBuf,
        catalog: Arc<CatalogService>,
        authorizer: Arc<dyn Authorizer>,
        playback: Arc<PlaybackService>,
        hw_accel: HwAccel,
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
            hw_accel,
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

/// Paramètres de la requête de transcodage.
#[derive(serde::Deserialize, Default)]
pub struct TranscodeQuery {
    /// Position de départ en secondes (défaut : 0).
    #[serde(default)]
    pub start: f64,
    /// Hauteur cible en pixels (défaut : 1080). Valeurs acceptées : 2160, 1080, 720, 480, 360.
    #[serde(default = "default_height")]
    pub height: u32,
}

fn default_height() -> u32 { 1080 }

/// GET /v1/media/:media_id/transcode?start=0&height=1080
///
/// Stratégie Direct Play / Transcode :
/// - Si le média est déjà H.264+AAC (ou H.264+MP3/Opus) → redirect 302 vers `/v1/media/:id`
///   (Direct Play sans transcodage).
/// - Sinon → lance ffmpeg et streame un fragmented MP4 en direct.
pub async fn transcode_media_handler(
    State(state): State<AppState>,
    Path(media_id): Path<norylum_foundation::MediaId>,
    Query(params): Query<TranscodeQuery>,
    headers: HeaderMap,
) -> Result<Response, NorylumError> {
    use axum::http::{header::LOCATION, StatusCode};

    let ctx = norylum_streaming::extract_request_context_from_headers(&headers);
    state.authorizer.authorize_request(&ctx).await?;

    let media = state
        .catalog
        .get_by_id(&media_id)
        .await
        .ok_or_else(|| NorylumError::NotFound(format!("Média '{}' introuvable", media_id)))?;

    let access_info = norylum_authorization::MediaAccessInfo::new(
        media.id.clone(),
        media.is_child_safe,
        media.age_rating,
    );
    state.authorizer.authorize_media_access(&ctx, &access_info).await?;

    // Direct Play si le codec vidéo est déjà H.264 et l'audio compatible navigateur.
    let is_direct_play = media.video_codec.as_deref() == Some("h264")
        && matches!(
            media.audio_codec.as_deref(),
            Some("aac") | Some("mp3") | Some("opus")
        );

    if is_direct_play && params.start == 0.0 {
        let location = format!("/v1/media/{}", media_id);
        return Ok(Response::builder()
            .status(StatusCode::FOUND)
            .header(LOCATION, location)
            .body(Body::empty())
            .map_err(|e| NorylumError::Internal(e.to_string()))?);
    }

    // Validation de la hauteur cible.
    let target_height = match params.height {
        h if [2160, 1080, 720, 480, 360].contains(&h) => h,
        _ => 1080,
    };

    let safe_path = norylum_library::PathSanitizer::sanitize_path(
        &state.library_root,
        &media.file_path,
    )?;

    let req = TranscodeRequest {
        input: &safe_path,
        start_secs: params.start.max(0.0),
        accel: state.hw_accel,
        target_height,
    };

    let mut child = norylum_transcoding::spawn_transcode(&req)
        .map_err(|e| NorylumError::Internal(format!("Lancement ffmpeg échoué : {e}")))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| NorylumError::Internal("ffmpeg stdout manquant".into()))?;

    // Le stream se termine naturellement quand ffmpeg finit ou quand le client se déconnecte.
    // `kill_on_drop(true)` sur le Child garantit que ffmpeg est tué si le stream est abandonné.
    let stream = ReaderStream::with_capacity(stdout, 64 * 1024);
    let body = Body::from_stream(stream);

    // Garder le child en vie jusqu'à la fin du stream via un task dédié.
    tokio::spawn(async move {
        let _ = child.wait().await;
    });

    tracing::info!(
        media_id = %media_id,
        accel = state.hw_accel.as_str(),
        start = params.start,
        height = target_height,
        "Transcodage démarré"
    );

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "video/mp4")
        .header("Cache-Control", "no-store")
        .header("X-Transcode-Accel", state.hw_accel.as_str())
        .body(body)
        .map_err(|e| NorylumError::Internal(e.to_string()))
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

    let poster_cache = Arc::new(PosterCache::new(
        state.library_root.join(".norylum-images/posters"),
    ));
    if let Err(e) = poster_cache.prepare_dir() {
        tracing::warn!(error = %e, "Impossible de creer le repertoire de cache des affiches");
    }

    let metadata_state = metadata_routes::MetadataState {
        catalog: state.catalog.clone(),
        authorizer: state.authorizer.clone(),
        db: state.playback.db().clone(),
        tmdb: TmdbClient::from_env(),
        poster_cache: poster_cache.clone(),
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

    let acquisition_routes = Router::new()
        .route(
            "/requests",
            axum::routing::get(acquisition_routes::list_handler)
                .post(acquisition_routes::create_handler),
        )
        .route(
            "/requests/:id",
            axum::routing::get(acquisition_routes::get_handler),
        )
        .route(
            "/requests/:id/approve",
            axum::routing::post(acquisition_routes::approve_handler),
        )
        .route(
            "/requests/:id/cancel",
            axum::routing::post(acquisition_routes::cancel_handler),
        )
        .route(
            "/requests/:id/source",
            axum::routing::post(acquisition_routes::register_source_handler),
        )
        .with_state(state.clone());

    let torrent_routes = Router::new()
        .route("/shares", axum::routing::get(torrent_routes::list_handler))
        .route(
            "/settings",
            axum::routing::get(torrent_routes::get_settings_handler)
                .put(torrent_routes::set_settings_handler),
        )
        .route(
            "/media/:media_id/enable",
            axum::routing::post(torrent_routes::enable_handler),
        )
        .route(
            "/media/:media_id/disable",
            axum::routing::post(torrent_routes::disable_handler),
        )
        .with_state(state.clone());

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
        .with_state(state.clone());

    let streaming_routes = Router::new()
        .route(
            "/:media_id",
            axum::routing::get(norylum_streaming::stream_media_handler),
        )
        .with_state(streaming_state);

    let transcode_routes = Router::new()
        .route(
            "/:media_id/transcode",
            axum::routing::get(transcode_media_handler),
        )
        .with_state(state);

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
        .nest("/v1/acquisition", acquisition_routes)
        .nest("/v1/torrent", torrent_routes)
        .nest("/v1/media", streaming_routes)
        .nest("/v1/media", transcode_routes)
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
        .layer(middleware::from_fn_with_state(
            auth.clone(),
            auth_routes::auth_middleware,
        ));

    let limiter = new_auth_rate_limiter();

    // /v1/auth/login — rate limitée, hors middleware d'authentification
    let rate_limited_login = Router::new()
        .route(
            "/v1/auth/login",
            axum::routing::post(auth_routes::login_handler).with_state(auth.clone()),
        )
        .layer(middleware::from_fn_with_state(
            limiter.clone(),
            auth_rate_limit,
        ));

    // /v1/auth/switch-profile — rate limitée puis authentifiée
    let rate_limited_switch = Router::new()
        .route(
            "/v1/auth/switch-profile",
            axum::routing::post(auth_routes::switch_profile_handler).with_state(auth.clone()),
        )
        .layer(middleware::from_fn_with_state(
            auth.clone(),
            auth_routes::auth_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            limiter,
            auth_rate_limit,
        ));

    Router::new()
        .route("/health", axum::routing::get(|| async { "OK" }))
        .route(
            "/v1/images/posters/:filename",
            axum::routing::get(serve_poster_handler).with_state(poster_cache),
        )
        .merge(rate_limited_login)
        .merge(rate_limited_switch)
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
            HwAccel::Software,
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
            HwAccel::Software,
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
