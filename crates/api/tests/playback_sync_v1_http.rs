use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    response::Response,
    Router,
};
use norylum_api::{create_router, AppState};
use norylum_authorization::DefaultAuthorizer;
use norylum_catalog::CatalogService;
use norylum_library::MediaItem;
use norylum_persistence::Database;
use norylum_playback::PlaybackService;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
use tower::ServiceExt;

const PASSWORD: &str = "playback-sync-test-password-2026";

fn media(id: &str, child_safe: bool, age: u32) -> MediaItem {
    MediaItem {
        id: id.into(),
        title: id.into(),
        file_path: PathBuf::from(format!("/tmp/{id}.mp4")),
        relative_path: format!("{id}.mp4"),
        file_size: 1000,
        duration_seconds: 100.0,
        mime_type: "video/mp4".into(),
        video_codec: None,
        audio_codec: None,
        width: None,
        height: None,
        is_child_safe: child_safe,
        age_rating: Some(age),
    }
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    payload: Option<Value>,
) -> Response {
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

async fn data(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();

    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn test_cross_device_resume_and_conflicts() {
    let db = Database::in_memory().unwrap();

    db.sync_media_items(&[media("med_kids", true, 6), media("med_adult", false, 18)])
        .await
        .unwrap();

    let catalog = Arc::new(CatalogService::new(db.clone()));
    let playback = Arc::new(PlaybackService::new(db));

    let state = AppState::new(
        std::env::temp_dir(),
        catalog,
        Arc::new(DefaultAuthorizer::new()),
        playback,
        norylum_transcoding::HwAccel::Software,
    );

    let auth = state.auth.clone();

    auth.bootstrap_admin("admin", PASSWORD).await.unwrap();

    let mac = auth
        .login_with_device("admin", PASSWORD, Some("Mac"))
        .await
        .unwrap();

    let tv = auth
        .login_with_device("admin", PASSWORD, Some("TV salon"))
        .await
        .unwrap();

    let child_id = auth
        .create_profile(&mac.account_id, "Junior", true, Some(10))
        .await
        .unwrap();

    let child = auth
        .switch_profile(&mac.access_token, &child_id)
        .await
        .unwrap();

    let app = create_router(state);

    // Aucune authentification : 401.
    let anonymous = call(&app, "GET", "/v1/playback/sync", None, None).await;

    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    // Mac cree la progression, revision 1.
    let first = call(
        &app,
        "PUT",
        "/v1/playback/sync/med_adult",
        Some(&mac.access_token),
        Some(json!({
            "position_seconds": 20.0,
            "duration_seconds": 100.0
        })),
    )
    .await;

    assert_eq!(first.status(), StatusCode::OK);

    let first = data(first).await;
    assert_eq!(first["revision"], 1);
    assert_eq!(first["position_seconds"], 20.0);

    // TV retrouve la progression du Mac.
    let tv_progress = call(
        &app,
        "GET",
        "/v1/playback/med_adult",
        Some(&tv.access_token),
        None,
    )
    .await;

    assert_eq!(tv_progress.status(), StatusCode::OK);
    assert_eq!(data(tv_progress).await["position_seconds"], 20.0);

    // TV modifie avec la revision 1.
    let second = call(
        &app,
        "PUT",
        "/v1/playback/sync/med_adult",
        Some(&tv.access_token),
        Some(json!({
            "position_seconds": 70.0,
            "duration_seconds": 100.0,
            "expected_revision": 1
        })),
    )
    .await;

    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(data(second).await["revision"], 2);

    // Mac est en retard : HTTP 409.
    let stale = call(
        &app,
        "PUT",
        "/v1/playback/sync/med_adult",
        Some(&mac.access_token),
        Some(json!({
            "position_seconds": 35.0,
            "duration_seconds": 100.0,
            "expected_revision": 1
        })),
    )
    .await;

    assert_eq!(stale.status(), StatusCode::CONFLICT);

    // La progression de 70s n'a pas ete ecrasee.
    let current = call(
        &app,
        "GET",
        "/v1/playback/med_adult",
        Some(&mac.access_token),
        None,
    )
    .await;

    assert_eq!(data(current).await["position_seconds"], 70.0);

    // Ancienne API : compatibilite, revision 3.
    let legacy = call(
        &app,
        "POST",
        "/v1/playback/progress",
        Some(&mac.access_token),
        Some(json!({
            "media_id": "med_adult",
            "position_seconds": 95.0,
            "duration_seconds": 100.0
        })),
    )
    .await;

    assert_eq!(legacy.status(), StatusCode::OK);

    let legacy = data(legacy).await;
    assert_eq!(legacy["revision"], 3);
    assert_eq!(legacy["is_completed"], true);

    // Film termine : pas dans continue-watching.
    let watching = call(
        &app,
        "GET",
        "/v1/playback/continue-watching",
        Some(&tv.access_token),
        None,
    )
    .await;

    assert_eq!(watching.status(), StatusCode::OK);
    assert_eq!(data(watching).await.as_array().unwrap().len(), 0);

    // Le profil enfant ne peut voir ni modifier le media adulte.
    let forbidden = call(
        &app,
        "GET",
        "/v1/playback/med_adult",
        Some(&child.access_token),
        None,
    )
    .await;

    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let denied_write = call(
        &app,
        "PUT",
        "/v1/playback/sync/med_adult",
        Some(&child.access_token),
        Some(json!({
            "position_seconds": 10.0,
            "duration_seconds": 100.0
        })),
    )
    .await;

    assert_eq!(denied_write.status(), StatusCode::FORBIDDEN);

    // Le profil enfant peut sauvegarder un media autorise.
    let kids = call(
        &app,
        "PUT",
        "/v1/playback/sync/med_kids",
        Some(&child.access_token),
        Some(json!({
            "position_seconds": 40.0,
            "duration_seconds": 100.0
        })),
    )
    .await;

    assert_eq!(kids.status(), StatusCode::OK);
    assert_eq!(data(kids).await["revision"], 1);

    // L'historique enfant est separe de celui des adultes.
    let child_list = call(
        &app,
        "GET",
        "/v1/playback/sync",
        Some(&child.access_token),
        None,
    )
    .await;

    let child_list = data(child_list).await;
    assert_eq!(child_list.as_array().unwrap().len(), 1);
    assert_eq!(child_list[0]["media_id"], "med_kids");

    let adult_list = call(
        &app,
        "GET",
        "/v1/playback/sync",
        Some(&mac.access_token),
        None,
    )
    .await;

    let adult_list = data(adult_list).await;
    assert_eq!(adult_list.as_array().unwrap().len(), 1);
    assert_eq!(adult_list[0]["media_id"], "med_adult");

    // La reprise enfant est disponible independamment.
    let child_watching = call(
        &app,
        "GET",
        "/v1/playback/continue-watching",
        Some(&child.access_token),
        None,
    )
    .await;

    let child_watching = data(child_watching).await;
    assert_eq!(child_watching.as_array().unwrap().len(), 1);
    assert_eq!(child_watching[0]["position_seconds"], 40.0);
}
