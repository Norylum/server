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

const PASSWORD: &str = "sync-v2-test-password-2026";

fn media(id: &str, safe: bool) -> MediaItem {
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
        is_child_safe: safe,
        age_rating: Some(if safe { 6 } else { 18 }),
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
async fn test_retry_conflict_history_and_child_access() {
    let db = Database::in_memory().unwrap();

    db.sync_media_items(&[media("med_movie", false), media("med_kids", true)])
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

    let mac = auth
        .login_with_device("admin", PASSWORD, Some("Mac"))
        .await
        .unwrap();

    let tv = auth
        .login_with_device("admin", PASSWORD, Some("TV"))
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
    let path = "/v1/playback/sync/med_movie";

    let first_payload = json!({
        "position_seconds": 20.0,
        "duration_seconds": 100.0,
        "operation_id": "operation-0001"
    });

    // Le Mac cree une progression.
    let first = call(
        &app,
        "PUT",
        path,
        Some(&mac.access_token),
        Some(first_payload.clone()),
    )
    .await;

    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(data(first).await["revision"], 1);

    // La reponse a ete perdue : le client rejoue.
    let retried = call(
        &app,
        "PUT",
        path,
        Some(&mac.access_token),
        Some(first_payload.clone()),
    )
    .await;

    assert_eq!(retried.status(), StatusCode::OK);
    assert_eq!(data(retried).await["revision"], 1);

    // La television met a jour le meme profil.
    let newer = call(
        &app,
        "PUT",
        path,
        Some(&tv.access_token),
        Some(json!({
            "position_seconds": 75.0,
            "duration_seconds": 100.0,
            "expected_revision": 1,
            "operation_id": "operation-0002"
        })),
    )
    .await;

    assert_eq!(newer.status(), StatusCode::OK);
    assert_eq!(data(newer).await["revision"], 2);

    // Un nouveau message Mac, devenu obsolete.
    let conflict = call(
        &app,
        "PUT",
        path,
        Some(&mac.access_token),
        Some(json!({
            "position_seconds": 40.0,
            "duration_seconds": 100.0,
            "expected_revision": 1,
            "operation_id": "operation-0003"
        })),
    )
    .await;

    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let conflict = data(conflict).await;
    assert_eq!(conflict["code"], "CONFLICT");
    assert_eq!(conflict["current"]["revision"], 2);
    assert_eq!(conflict["current"]["position_seconds"], 75.0);

    // Rejouer la vieille operation ne retablit pas 20 s.
    let old = call(
        &app,
        "PUT",
        path,
        Some(&mac.access_token),
        Some(first_payload),
    )
    .await;

    assert_eq!(old.status(), StatusCode::OK);
    assert_eq!(data(old).await["revision"], 1);

    let current = call(
        &app,
        "GET",
        "/v1/playback/med_movie",
        Some(&mac.access_token),
        None,
    )
    .await;

    assert_eq!(data(current).await["position_seconds"], 75.0);

    // Meme ID, autre contenu : rejet.
    let reused = call(
        &app,
        "PUT",
        path,
        Some(&mac.access_token),
        Some(json!({
            "position_seconds": 55.0,
            "duration_seconds": 100.0,
            "operation_id": "operation-0001"
        })),
    )
    .await;

    assert_eq!(reused.status(), StatusCode::BAD_REQUEST);

    let history = call(
        &app,
        "GET",
        "/v1/playback/history",
        Some(&tv.access_token),
        None,
    )
    .await;

    assert_eq!(history.status(), StatusCode::OK);

    let history = data(history).await;
    assert_eq!(history.as_array().unwrap().len(), 1);
    assert_eq!(history[0]["media_id"], "med_movie");

    let denied = call(&app, "GET", "/v1/playback/history", None, None).await;

    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

    // L'enfant ne voit pas le film pour adultes.
    let child_history = call(
        &app,
        "GET",
        "/v1/playback/history",
        Some(&child.access_token),
        None,
    )
    .await;

    assert_eq!(child_history.status(), StatusCode::OK);
    assert_eq!(data(child_history).await.as_array().unwrap().len(), 0);

    let child_denied = call(
        &app,
        "PUT",
        path,
        Some(&child.access_token),
        Some(json!({
            "position_seconds": 10.0,
            "duration_seconds": 100.0,
            "operation_id": "operation-child-denied"
        })),
    )
    .await;

    assert_eq!(child_denied.status(), StatusCode::FORBIDDEN);
}
