//! Test d'intégration complet de la tranche verticale :
//! Scan Média -> Catalogue (Movies & Shows) -> Épisodes -> Streaming Direct Play 206 -> Progression Playback.
//! Valide la persistance SQLite local-first (`norylum.sqlite3`), la survie aux redémarrages et la stabilité des MediaId.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use norylum_api::{create_router, AppState};
use norylum_authorization::DefaultAuthorizer;
use norylum_catalog::{CatalogService, SeasonDetails, ShowDetails, ShowRecord};
use norylum_library::MediaItem;
use norylum_persistence::Database;
use norylum_playback::{PlaybackProgress, PlaybackService};
use std::io::Write;
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn test_full_vertical_slice() {
    // 1. Mise en place du répertoire média temporaire
    let temp_root = std::env::temp_dir().join(format!("norylum_e2e_{}", uuid::Uuid::new_v4()));
    let cartoon_dir = temp_root.join("dessins_animes");
    std::fs::create_dir_all(&cartoon_dir).unwrap();

    // Film enfant
    let kids_video = cartoon_dir.join("super_lapin (2020).mp4");
    let mut file1 = std::fs::File::create(&kids_video).unwrap();
    let video_bytes = b"VIDEO_DIRECT_PLAY_STREAMING_DATA_0123456789";
    file1.write_all(video_bytes).unwrap();

    // Film adulte
    let adult_video = temp_root.join("film_action_adult_18+ (2018).mp4");
    let mut file2 = std::fs::File::create(&adult_video).unwrap();
    file2.write_all(b"ADULT_CONTENT_RESTRICTED").unwrap();

    // Série TV : Bécé Bégé S01E01
    let show_dir = temp_root.join("Bece_Bege").join("Saison 01");
    std::fs::create_dir_all(&show_dir).unwrap();
    let ep_video = show_dir.join("Bece_Bege - S01E01 - Tel est pris.mp4");
    let mut file3 = std::fs::File::create(&ep_video).unwrap();
    let ep_bytes = b"EPISODE_DIRECT_PLAY_STREAMING_DATA_9876543210";
    file3.write_all(ep_bytes).unwrap();

    // 2. Initialisation de la persistance SQLite local-first sur disque
    let canonical_root = std::fs::canonicalize(&temp_root).unwrap();
    let db_path = canonical_root.join("norylum.sqlite3");
    let db = Database::open(&db_path).unwrap();

    let catalog = Arc::new(CatalogService::new(db.clone()));
    let authorizer = Arc::new(DefaultAuthorizer::new());
    let playback = Arc::new(PlaybackService::new(db.clone()));

    let state = AppState::new(
        canonical_root.clone(),
        catalog.clone(),
        authorizer.clone(),
        playback.clone(),
    );

    let app = create_router(state);

    // ---------------------------------------------------------
    // ÉTAPE A : Scan Média (/v1/library/scan)
    // ---------------------------------------------------------
    // Cas 1 : Non-authentifié -> Rejet Fail-Closed (401)
    let unauth_scan_req = Request::builder()
        .method("POST")
        .uri("/v1/library/scan")
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(unauth_scan_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Cas 2 : Authentifié -> Scan réussi et persisté dans SQLite
    let auth_scan_req = Request::builder()
        .method("POST")
        .uri("/v1/library/scan")
        .header("x-account-id", "acc_famille")
        .header("x-profile-id", "prof_admin")
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(auth_scan_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let scan_res: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(scan_res["scanned_count"], 3);

    // ---------------------------------------------------------
    // ÉTAPE B : Catalogue Global (/v1/library/media)
    // ---------------------------------------------------------
    // Profil Enfant : voit les 2 médias enfants (film enfant + épisode)
    let child_media_req = Request::builder()
        .method("GET")
        .uri("/v1/library/media")
        .header("x-account-id", "acc_famille")
        .header("x-profile-id", "prof_enfant")
        .header("x-is-child", "true")
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(child_media_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let child_media_list: Vec<MediaItem> = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(child_media_list.len(), 2);
    let kids_media_id = child_media_list
        .iter()
        .find(|m| m.title.contains("super"))
        .unwrap()
        .id
        .clone();

    // Profil Adulte : doit voir les 3 médias
    let adult_media_req = Request::builder()
        .method("GET")
        .uri("/v1/library/media")
        .header("x-account-id", "acc_famille")
        .header("x-profile-id", "prof_parent")
        .header("x-is-child", "false")
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(adult_media_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let adult_media_list: Vec<MediaItem> = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(adult_media_list.len(), 3);
    let adult_media_id = adult_media_list
        .iter()
        .find(|m| m.title.contains("adult"))
        .unwrap()
        .id
        .clone();

    // ---------------------------------------------------------
    // ÉTAPE C : Catalogue Films (/v1/library/movies)
    // ---------------------------------------------------------
    let child_movies_req = Request::builder()
        .method("GET")
        .uri("/v1/library/movies")
        .header("x-account-id", "acc_famille")
        .header("x-profile-id", "prof_enfant")
        .header("x-is-child", "true")
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(child_movies_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let child_movies: Vec<MediaItem> = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(child_movies.len(), 1);
    assert!(child_movies[0].title.contains("super"));

    // ---------------------------------------------------------
    // ÉTAPE D : Séries, Saisons & Épisodes (/v1/library/shows)
    // ---------------------------------------------------------
    // 1. Liste des séries
    let shows_req = Request::builder()
        .method("GET")
        .uri("/v1/library/shows")
        .header("x-account-id", "acc_famille")
        .header("x-profile-id", "prof_enfant")
        .header("x-is-child", "true")
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(shows_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let shows: Vec<ShowRecord> = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(shows.len(), 1);
    assert_eq!(shows[0].title, "Bece Bege");
    assert_eq!(shows[0].season_count, 1);
    assert_eq!(shows[0].episode_count, 1);
    let show_id = shows[0].id.clone();

    // 2. Détail de la série
    let show_detail_req = Request::builder()
        .method("GET")
        .uri(format!("/v1/library/shows/{}", show_id))
        .header("x-account-id", "acc_famille")
        .header("x-profile-id", "prof_enfant")
        .header("x-is-child", "true")
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(show_detail_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let show_detail: ShowDetails = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(show_detail.seasons.len(), 1);
    assert_eq!(show_detail.seasons[0].season_number, 1);

    // 3. Détail de la saison avec liste des épisodes
    let season_detail_req = Request::builder()
        .method("GET")
        .uri(format!("/v1/library/shows/{}/seasons/1", show_id))
        .header("x-account-id", "acc_famille")
        .header("x-profile-id", "prof_enfant")
        .header("x-is-child", "true")
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(season_detail_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let season_detail: SeasonDetails = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(season_detail.episodes.len(), 1);
    assert_eq!(season_detail.episodes[0].episode_number, 1);
    assert_eq!(season_detail.episodes[0].title, "Tel est pris");
    let ep_media_id = season_detail.episodes[0].media_id.clone();

    // ---------------------------------------------------------
    // ÉTAPE E : Streaming Direct Play 206 (/v1/media/:media_id)
    // ---------------------------------------------------------
    // 1. Sécurité enfant : L'enfant tente d'accéder au média adulte -> 403 Forbidden
    let forbidden_stream_req = Request::builder()
        .method("GET")
        .uri(format!("/v1/media/{}", adult_media_id))
        .header("x-account-id", "acc_famille")
        .header("x-profile-id", "prof_enfant")
        .header("x-is-child", "true")
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(forbidden_stream_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // 2. Direct Play avec Range HTTP sur le film enfant : Requête des 10 premiers octets (0-9)
    let range_stream_req = Request::builder()
        .method("GET")
        .uri(format!("/v1/media/{}", kids_media_id))
        .header("x-account-id", "acc_famille")
        .header("x-profile-id", "prof_enfant")
        .header("x-is-child", "true")
        .header(header::RANGE, "bytes=0-9")
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(range_stream_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(resp.headers().get(header::ACCEPT_RANGES).unwrap(), "bytes");
    assert_eq!(
        resp.headers().get(header::CONTENT_RANGE).unwrap(),
        &format!("bytes 0-9/{}", video_bytes.len())
    );
    assert_eq!(resp.headers().get(header::CONTENT_LENGTH).unwrap(), "10");

    let chunk_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&chunk_bytes[..], &video_bytes[0..10]);

    // 3. Streaming Direct Play sur l'épisode de série (Range 206)
    let ep_stream_req = Request::builder()
        .method("GET")
        .uri(format!("/v1/media/{}", ep_media_id))
        .header("x-account-id", "acc_famille")
        .header("x-profile-id", "prof_enfant")
        .header("x-is-child", "true")
        .header(header::RANGE, "bytes=0-9")
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(ep_stream_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        resp.headers().get(header::CONTENT_RANGE).unwrap(),
        &format!("bytes 0-9/{}", ep_bytes.len())
    );
    let ep_chunk = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&ep_chunk[..], &ep_bytes[0..10]);

    // ---------------------------------------------------------
    // ÉTAPE F : Progression Playback sur l'épisode (/v1/playback)
    // ---------------------------------------------------------
    let update_progress_req = Request::builder()
        .method("POST")
        .uri("/v1/playback/progress")
        .header("x-account-id", "acc_famille")
        .header("x-profile-id", "prof_enfant")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_string(&serde_json::json!({
                "media_id": ep_media_id.as_str(),
                "position_seconds": 95.0,
                "duration_seconds": 100.0
            }))
            .unwrap(),
        ))
        .unwrap();

    let resp = app.clone().oneshot(update_progress_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let saved_progress: PlaybackProgress = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(saved_progress.position_seconds, 95.0);
    assert!(saved_progress.is_completed);

    // ---------------------------------------------------------
    // ÉTAPE G : Redémarrage Serveur et Persistance SQLite
    // ---------------------------------------------------------
    drop(app);
    drop(catalog);
    drop(playback);
    drop(db);

    // Réouverture de la base existante
    let reboot_db = Database::open(&db_path).unwrap();
    let reboot_catalog = Arc::new(CatalogService::new(reboot_db.clone()));
    let reboot_playback = Arc::new(PlaybackService::new(reboot_db.clone()));

    let reboot_state = AppState::new(
        canonical_root.clone(),
        reboot_catalog.clone(),
        authorizer.clone(),
        reboot_playback.clone(),
    );
    let reboot_app = create_router(reboot_state);

    // 1. Séries et saisons toujours présentes sans avoir rescanné
    let reboot_shows_req = Request::builder()
        .method("GET")
        .uri("/v1/library/shows")
        .header("x-account-id", "acc_famille")
        .header("x-profile-id", "prof_parent")
        .header("x-is-child", "false")
        .body(Body::empty())
        .unwrap();

    let resp = reboot_app.clone().oneshot(reboot_shows_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let reboot_shows: Vec<ShowRecord> = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(reboot_shows.len(), 1);
    assert_eq!(reboot_shows[0].title, "Bece Bege");

    // 2. Progression de l'épisode intacte après redémarrage
    let progress_after_reboot = Request::builder()
        .method("GET")
        .uri(format!("/v1/playback/{}", ep_media_id))
        .header("x-account-id", "acc_famille")
        .header("x-profile-id", "prof_enfant")
        .body(Body::empty())
        .unwrap();

    let resp = reboot_app.oneshot(progress_after_reboot).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let reloaded_prog: PlaybackProgress = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(reloaded_prog.media_id, ep_media_id);
    assert_eq!(reloaded_prog.position_seconds, 95.0);
    assert!(reloaded_prog.is_completed);

    // Nettoyage
    let _ = std::fs::remove_dir_all(temp_root);
}
