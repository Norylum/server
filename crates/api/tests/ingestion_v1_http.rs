use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use norylum_api::{create_router, AppState};
use norylum_authorization::DefaultAuthorizer;
use norylum_catalog::CatalogService;
use norylum_persistence::Database;
use norylum_playback::PlaybackService;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn ready_requires_auth_principal_and_valid_paths() {
    let db = Database::in_memory().unwrap();
    let state = AppState::new(
        std::env::temp_dir(),
        Arc::new(CatalogService::new(db.clone())),
        Arc::new(DefaultAuthorizer::new()),
        Arc::new(PlaybackService::new(db)),
    );
    let auth = state.auth.clone();
    auth.bootstrap_admin("admin", "ingestion-test-password-2026")
        .await
        .unwrap();
    let adult = auth
        .login("admin", "ingestion-test-password-2026")
        .await
        .unwrap();
    let child_id = auth
        .create_profile(&adult.account_id, "Junior", true, Some(10))
        .await
        .unwrap();
    let child = auth
        .switch_profile(&adult.access_token, &child_id)
        .await
        .unwrap();

    let app = create_router(state);
    let payload = json!({
        "source":"../outside.mkv",
        "destination":"Movies/Example (2020).mkv"
    });
    let url = "/v1/library/ingest/ready";

    let unauthenticated = app
        .clone()
        .oneshot(
            Request::post(url)
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let forbidden = app
        .clone()
        .oneshot(
            Request::post(url)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", child.access_token))
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let invalid = app
        .oneshot(
            Request::post(url)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", adult.access_token))
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
}
