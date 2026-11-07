use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use norylum_api::{create_router, AppState};
use norylum_authorization::DefaultAuthorizer;
use norylum_catalog::CatalogService;
use norylum_foundation::NorylumError;
use norylum_library::MediaItem;
use norylum_metadata::{poster_url, TmdbItem, TmdbKind};
use norylum_persistence::Database;
use norylum_playback::PlaybackService;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
use tower::ServiceExt;

const PASSWORD: &str = "metadata-test-password-2026";

fn movie() -> MediaItem {
    MediaItem {
        id: "med_metadata_test".into(),
        title: "Film Test".into(),
        file_path: PathBuf::from("/tmp/Film Test (2000).mkv"),
        relative_path: "Film Test (2000).mkv".into(),
        file_size: 1000,
        duration_seconds: 100.0,
        mime_type: "video/x-matroska".into(),
        video_codec: None,
        audio_codec: None,
        width: None,
        height: None,
        is_child_safe: false,
        age_rating: Some(18),
    }
}

fn cached_item(kind: TmdbKind) -> TmdbItem {
    TmdbItem {
        tmdb_id: 603,
        tmdb_ref: format!("{}/603", kind.as_str()),
        kind,
        title: "Titre TMDB".into(),
        overview: "Synopsis de test".into(),
        poster_path: Some("/cover123.jpg".into()),
        poster_url: poster_url("/cover123.jpg"),
        release_date: Some("2000-01-01".into()),
    }
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    payload: Option<Value>,
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(path);

    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }

    let body = if let Some(payload) = payload {
        builder = builder.header("content-type", "application/json");
        Body::from(payload.to_string())
    } else {
        Body::empty()
    };

    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

async fn body(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();

    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn test_metadata_cached_and_profile_isolation() {
    let db = Database::in_memory().unwrap();
    let item = movie();

    db.sync_media_items(std::slice::from_ref(&item))
        .await
        .unwrap();

    db.set_movie_metadata(&item.id, &cached_item(TmdbKind::Movie))
        .await
        .unwrap();

    // Serie factice dans la base, sans acces reseau.
    db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO shows (
                id, title, year, relative_path,
                is_child_safe, age_rating,
                created_at, updated_at
             ) VALUES (
                'shw_metadata_test', 'Serie Test', 2000,
                'Serie Test', 0, 18, 'now', 'now'
             )",
            [],
        )
        .map_err(|e| NorylumError::Internal(e.to_string()))?;

        Ok(())
    })
    .await
    .unwrap();

    db.set_show_metadata(&"shw_metadata_test".into(), &cached_item(TmdbKind::Tv))
        .await
        .unwrap();

    let state = AppState::new(
        std::env::temp_dir(),
        Arc::new(CatalogService::new(db.clone())),
        Arc::new(DefaultAuthorizer::new()),
        Arc::new(PlaybackService::new(db)),
        norylum_transcoding::HwAccel::Software,
    );

    let auth = state.auth.clone();

    auth.bootstrap_admin("admin", PASSWORD).await.unwrap();

    let adult = auth.login("admin", PASSWORD).await.unwrap();

    let child_id = auth
        .create_profile(&adult.account_id, "Junior", true, Some(10))
        .await
        .unwrap();

    let child = auth
        .switch_profile(&adult.access_token, &child_id)
        .await
        .unwrap();

    let app = create_router(state);

    let film_path = "/v1/metadata/media/med_metadata_test";

    let unauthenticated = call(&app, "GET", film_path, None, None).await;

    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let film = call(&app, "GET", film_path, Some(&adult.access_token), None).await;

    assert_eq!(film.status(), StatusCode::OK);

    let film = body(film).await;

    assert_eq!(film["tmdb_id"], 603);
    assert_eq!(film["tmdb_ref"], "movie/603");
    assert_eq!(film["provenance"], "Norylum Server");
    assert_eq!(film["source"], "TMDB");

    assert_eq!(
        film["poster_url"],
        "https://image.tmdb.org/t/p/w500/cover123.jpg"
    );

    let child_film = call(&app, "GET", film_path, Some(&child.access_token), None).await;

    assert_eq!(child_film.status(), StatusCode::FORBIDDEN);

    let child_edit = call(
        &app,
        "PUT",
        film_path,
        Some(&child.access_token),
        Some(json!({"tmdb_id": 603})),
    )
    .await;

    assert_eq!(child_edit.status(), StatusCode::FORBIDDEN);

    let child_search = call(
        &app,
        "GET",
        "/v1/metadata/search?kind=movie&query=Matrix",
        Some(&child.access_token),
        None,
    )
    .await;

    assert_eq!(child_search.status(), StatusCode::FORBIDDEN);

    let show_path = "/v1/metadata/shows/shw_metadata_test";

    let show = call(&app, "GET", show_path, Some(&adult.access_token), None).await;

    assert_eq!(show.status(), StatusCode::OK);
    assert_eq!(body(show).await["tmdb_ref"], "tv/603");

    let child_show = call(&app, "GET", show_path, Some(&child.access_token), None).await;

    assert_eq!(child_show.status(), StatusCode::FORBIDDEN);

    let missing = call(
        &app,
        "GET",
        "/v1/metadata/media/med_unknown",
        Some(&adult.access_token),
        None,
    )
    .await;

    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    // Validation locale avant tout appel TMDB.
    let invalid = call(
        &app,
        "PUT",
        film_path,
        Some(&adult.access_token),
        Some(json!({"tmdb_id": 0})),
    )
    .await;

    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
}
