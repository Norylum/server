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
        norylum_transcoding::HwAccel::Software,
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

#[tokio::test]
async fn acquisition_v2_approval_and_cancellation() {
    let db = Database::in_memory().unwrap();
    let state = AppState::new(
        std::env::temp_dir(),
        Arc::new(CatalogService::new(db.clone())),
        Arc::new(DefaultAuthorizer::new()),
        Arc::new(PlaybackService::new(db)),
        norylum_transcoding::HwAccel::Software,
    );

    let auth = state.auth.clone();
    let password = "acquisition-v2-test-password-2026";

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

    let payload = json!({
        "kind": "movie",
        "tmdb_id": 987654321,
        "quality_profile": "1080p",
        "language": "fr"
    });

    let (status, pending) = call(
        &app,
        "POST",
        endpoint,
        Some(&adult.access_token),
        Some(payload.clone()),
    )
    .await;

    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(pending["status"], "awaiting_approval");

    let id = pending["id"].as_str().unwrap().to_owned();
    let approve = format!("{endpoint}/{id}/approve");
    let cancel = format!("{endpoint}/{id}/cancel");

    assert_eq!(
        call(&app, "POST", &approve, None, None).await.0,
        StatusCode::UNAUTHORIZED
    );

    assert_eq!(
        call(&app, "POST", &approve, Some(&child.access_token), None,)
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    let (status, approved) = call(&app, "POST", &approve, Some(&adult.access_token), None).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(approved["status"], "approved");

    // Reapproval is a conflict, not an implicit success.
    assert_eq!(
        call(&app, "POST", &approve, Some(&adult.access_token), None,)
            .await
            .0,
        StatusCode::CONFLICT
    );

    let (status, cancelled) = call(&app, "POST", &cancel, Some(&adult.access_token), None).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(cancelled["status"], "cancelled");

    assert_eq!(
        call(&app, "POST", &cancel, Some(&adult.access_token), None,)
            .await
            .0,
        StatusCode::CONFLICT
    );

    assert_eq!(
        call(&app, "POST", &approve, Some(&adult.access_token), None,)
            .await
            .0,
        StatusCode::CONFLICT
    );

    let (status, fresh) = call(
        &app,
        "POST",
        endpoint,
        Some(&adult.access_token),
        Some(payload),
    )
    .await;

    assert_eq!(status, StatusCode::ACCEPTED);
    assert_ne!(fresh["id"], pending["id"]);
    assert_eq!(fresh["status"], "awaiting_approval");

    let fresh_cancel = format!("{endpoint}/{}/cancel", fresh["id"].as_str().unwrap());

    let (status, cancelled_pending) =
        call(&app, "POST", &fresh_cancel, Some(&adult.access_token), None).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(cancelled_pending["status"], "cancelled");

    // A nonexistent but well-formed request must return 404.
    let missing = format!("{endpoint}/acq_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/approve");

    assert_eq!(
        call(&app, "POST", &missing, Some(&adult.access_token), None,)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}
