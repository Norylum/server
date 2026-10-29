//! Test d'intégration complet de la tranche verticale :
//! Scan Média -> Catalogue -> Streaming Direct Play 206 -> Progression Playback.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use norylum_api::{create_router, AppState};
use norylum_authorization::DefaultAuthorizer;
use norylum_catalog::CatalogService;
use norylum_library::MediaItem;
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

    let kids_video = cartoon_dir.join("super_lapin.mp4");
    let mut file1 = std::fs::File::create(&kids_video).unwrap();
    let video_bytes = b"VIDEO_DIRECT_PLAY_STREAMING_DATA_0123456789";
    file1.write_all(video_bytes).unwrap();

    let adult_video = temp_root.join("film_action_adult_18+.mp4");
    let mut file2 = std::fs::File::create(&adult_video).unwrap();
    file2.write_all(b"ADULT_CONTENT_RESTRICTED").unwrap();

    // 2. Initialisation de l'application Monolithe Norylum
    let canonical_root = std::fs::canonicalize(&temp_root).unwrap();
    let catalog = Arc::new(CatalogService::new());
    let authorizer = Arc::new(DefaultAuthorizer::new());
    let playback = Arc::new(PlaybackService::new());

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

    // Cas 2 : Authentifié -> Scan réussi
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
    assert_eq!(scan_res["scanned_count"], 2);

    // ---------------------------------------------------------
    // ÉTAPE B : Catalogue (/v1/library/media)
    // ---------------------------------------------------------
    // Profil Enfant : ne doit voir QUE `super_lapin.mp4`
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
    assert_eq!(child_media_list.len(), 1);
    assert_eq!(child_media_list[0].title, "super_lapin");
    let kids_media_id = child_media_list[0].id.clone();

    // Profil Adulte : doit voir les 2 médias
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
    assert_eq!(adult_media_list.len(), 2);
    let adult_media_id = adult_media_list
        .iter()
        .find(|m| m.title.contains("adult"))
        .unwrap()
        .id
        .clone();

    // ---------------------------------------------------------
    // ÉTAPE C : Streaming Direct Play 206 (/v1/media/:media_id)
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

    // 2. Direct Play avec Range HTTP : Requête des 10 premiers octets (0-9)
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

    // 3. Fast-forward seek : Requête des octets 10-24
    let seek_stream_req = Request::builder()
        .method("GET")
        .uri(format!("/v1/media/{}", kids_media_id))
        .header("x-account-id", "acc_famille")
        .header("x-profile-id", "prof_enfant")
        .header("x-is-child", "true")
        .header(header::RANGE, "bytes=10-24")
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(seek_stream_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(resp.headers().get(header::CONTENT_LENGTH).unwrap(), "15");
    let chunk_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&chunk_bytes[..], &video_bytes[10..25]);

    // ---------------------------------------------------------
    // ÉTAPE D : Progression Playback (/v1/playback)
    // ---------------------------------------------------------
    // 1. Enregistrement de progression (95% lu -> is_completed = true)
    let update_progress_req = Request::builder()
        .method("POST")
        .uri("/v1/playback/progress")
        .header("x-account-id", "acc_famille")
        .header("x-profile-id", "prof_enfant")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_string(&serde_json::json!({
                "media_id": kids_media_id.as_str(),
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

    // 2. Lecture de la progression
    let get_progress_req = Request::builder()
        .method("GET")
        .uri(format!("/v1/playback/{}", kids_media_id))
        .header("x-account-id", "acc_famille")
        .header("x-profile-id", "prof_enfant")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(get_progress_req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let read_progress: PlaybackProgress = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(read_progress.media_id, kids_media_id);
    assert_eq!(read_progress.position_seconds, 95.0);

    // Nettoyage
    let _ = std::fs::remove_dir_all(temp_root);
}

