use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use norylum_api::{create_router, AppState};
use norylum_authorization::DefaultAuthorizer;
use norylum_catalog::CatalogService;
use norylum_persistence::Database;
use norylum_playback::PlaybackService;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    payload: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);

    if let Some(token) = token {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }

    let body = if let Some(value) = payload {
        builder = builder.header("Content-Type", "application/json");
        Body::from(value.to_string())
    } else {
        Body::empty()
    };

    let response = app
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();

    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();

    let data = serde_json::from_slice(&bytes).unwrap_or(Value::Null);

    (status, data)
}

#[tokio::test]
async fn acquisition_requires_principal_and_is_deduplicated() {
    let db = Database::in_memory().unwrap();

    let state = AppState::new(
        std::env::temp_dir(),
        Arc::new(CatalogService::new(db.clone())),
        Arc::new(DefaultAuthorizer::new()),
        Arc::new(PlaybackService::new(db)),
    );

    let auth = state.auth.clone();
    let password = "acquisition-test-password-2026";

    auth.bootstrap_admin("admin", password).await.unwrap();

    let adult = auth.login("admin", password).await.unwrap();

    let child_id = auth
        .create_profile(&adult.account_id, "Junior", true, Some(10))
        .await
        .unwrap();

    let child = auth
        .switch_profile(&adult.access_token, &child_id)
        .await
        .unwrap();

    let app = create_router(state);

    let endpoint = "/v1/acquisition/requests";
    let movie = json!({
        "kind": "movie",
        "tmdb_id": 603,
        "quality_profile": "1080p",
        "language": "fr"
    });

    let (status, _) = call(&app, "POST", endpoint, None, Some(movie.clone())).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = call(
        &app,
        "POST",
        endpoint,
        Some(&child.access_token),
        Some(movie.clone()),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, first) = call(
        &app,
        "POST",
        endpoint,
        Some(&adult.access_token),
        Some(movie.clone()),
    )
    .await;

    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(first["status"], "awaiting_approval");
    assert_eq!(first["tmdb_id"], 603);

    let (status, second) = call(
        &app,
        "POST",
        endpoint,
        Some(&adult.access_token),
        Some(movie),
    )
    .await;

    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(first["id"], second["id"]);

    let (status, requests) = call(&app, "GET", endpoint, Some(&adult.access_token), None).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(requests.as_array().unwrap().len(), 1);

    let id = first["id"].as_str().unwrap();
    let url = format!("{endpoint}/{id}");

    let (status, found) = call(&app, "GET", &url, Some(&adult.access_token), None).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(found["id"], id);

    let (status, _) = call(
        &app,
        "POST",
        endpoint,
        Some(&adult.access_token),
        Some(json!({"kind":"movie","tmdb_id":0})),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = call(
        &app,
        "POST",
        endpoint,
        Some(&adult.access_token),
        Some(json!({
            "kind":"tv",
            "tmdb_id":1396,
            "episode":1
        })),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, episode) = call(
        &app,
        "POST",
        endpoint,
        Some(&adult.access_token),
        Some(json!({
            "kind":"tv",
            "tmdb_id":1396,
            "season":1,
            "episode":2
        })),
    )
    .await;

    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(episode["season"], 1);
    assert_eq!(episode["episode"], 2);
    assert_eq!(episode["status"], "awaiting_approval");
}
