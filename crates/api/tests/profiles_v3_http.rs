use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    response::Response,
    Router,
};
use norylum_api::{create_router, AppState};
use norylum_authorization::DefaultAuthorizer;
use norylum_catalog::CatalogService;
use norylum_playback::PlaybackService;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const PASSWORD: &str = "http-pin-test-password-2026";
const PIN: &str = "736291";

async fn call(app: &Router, method: &str, path: &str, token: &str, data: Value) -> Response {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(data.to_string()))
        .unwrap();

    app.clone().oneshot(request).await.unwrap()
}

#[tokio::test]
async fn test_pin_http_routes() {
    let state = AppState::new(
        std::env::temp_dir(),
        Arc::new(CatalogService::in_memory()),
        Arc::new(DefaultAuthorizer::new()),
        Arc::new(PlaybackService::in_memory()),
    );

    let auth = state.auth.clone();

    auth.bootstrap_admin("admin", PASSWORD).await.unwrap();

    let admin = auth.login("admin", PASSWORD).await.unwrap();

    let adult = auth
        .create_profile(&admin.account_id, "Cinema", false, None)
        .await
        .unwrap();

    let app = create_router(state);

    let path = format!("/v1/profiles/{}/pin", adult.as_str());

    let denied = call(
        &app,
        "PUT",
        &path,
        &admin.access_token,
        json!({
            "pin": PIN,
            "account_password": "wrong"
        }),
    )
    .await;

    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let configured = call(
        &app,
        "PUT",
        &path,
        &admin.access_token,
        json!({
            "pin": PIN,
            "account_password": PASSWORD
        }),
    )
    .await;

    assert_eq!(configured.status(), StatusCode::NO_CONTENT);

    let switched = call(
        &app,
        "POST",
        "/v1/auth/switch-profile",
        &admin.access_token,
        json!({
            "profile_id": adult.as_str(),
            "profile_pin": PIN
        }),
    )
    .await;

    assert_eq!(switched.status(), StatusCode::OK);

    let body = to_bytes(switched.into_body(), 1024 * 1024).await.unwrap();

    let response: Value = serde_json::from_slice(&body).unwrap();

    let issued_token = response["access_token"].as_str().unwrap();

    let deleted = call(
        &app,
        "DELETE",
        &path,
        &admin.access_token,
        json!({
            "account_password": PASSWORD
        }),
    )
    .await;

    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

    let revoked = Request::builder()
        .uri("/v1/auth/me")
        .header("authorization", format!("Bearer {issued_token}"))
        .body(Body::empty())
        .unwrap();

    assert_eq!(
        app.oneshot(revoked).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}
