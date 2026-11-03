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

const PASSWORD: &str = "devices-v2-http-test-password";

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    data: Option<Value>,
) -> Response {
    let mut builder = Request::builder().method(method).uri(path);

    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }

    let body = if let Some(data) = data {
        builder = builder.header("content-type", "application/json");
        Body::from(data.to_string())
    } else {
        Body::empty()
    };

    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

async fn json_body(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();

    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn test_devices_v2_http_lifecycle() {
    let state = AppState::new(
        std::env::temp_dir(),
        Arc::new(CatalogService::in_memory()),
        Arc::new(DefaultAuthorizer::new()),
        Arc::new(PlaybackService::in_memory()),
    );

    let auth = state.auth.clone();

    auth.bootstrap_admin("admin", PASSWORD).await.unwrap();

    let admin = auth
        .login_with_device("admin", PASSWORD, Some("Mac"))
        .await
        .unwrap();

    let tv = auth
        .login_with_device("admin", PASSWORD, Some("TV"))
        .await
        .unwrap();

    let child_id = auth
        .create_profile(&admin.account_id, "Junior", true, Some(10))
        .await
        .unwrap();

    let child = auth
        .switch_profile(&admin.access_token, &child_id)
        .await
        .unwrap();

    let app = create_router(state);

    let path = format!("/v1/devices/{}", tv.device_id.as_str());

    let anonymous = call(&app, "PATCH", &path, None, Some(json!({"name": "Test"}))).await;

    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let forbidden = call(
        &app,
        "PATCH",
        &path,
        Some(&child.access_token),
        Some(json!({"name": "Interdit"})),
    )
    .await;

    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let invalid = call(
        &app,
        "PATCH",
        &path,
        Some(&admin.access_token),
        Some(json!({"name": "  "})),
    )
    .await;

    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

    let renamed = call(
        &app,
        "PATCH",
        &path,
        Some(&admin.access_token),
        Some(json!({
            "name": " Television du salon "
        })),
    )
    .await;

    assert_eq!(renamed.status(), StatusCode::NO_CONTENT);

    let devices = call(&app, "GET", "/v1/devices", Some(&admin.access_token), None).await;

    assert_eq!(devices.status(), StatusCode::OK);

    let listed = json_body(devices).await;

    assert!(listed
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == tv.device_id.as_str() && item["name"] == "Television du salon"));

    let history = format!("{path}/history");

    // Refus d'oublier un appareil encore connecte.
    let blocked = call(&app, "DELETE", &history, Some(&admin.access_token), None).await;

    assert_eq!(blocked.status(), StatusCode::BAD_REQUEST);

    let child_maintenance = call(
        &app,
        "POST",
        "/v1/devices/maintenance",
        Some(&child.access_token),
        None,
    )
    .await;

    assert_eq!(child_maintenance.status(), StatusCode::FORBIDDEN);

    let maintenance = call(
        &app,
        "POST",
        "/v1/devices/maintenance",
        Some(&admin.access_token),
        None,
    )
    .await;

    assert_eq!(maintenance.status(), StatusCode::OK);

    let report = json_body(maintenance).await;

    assert_eq!(report["expired_sessions_removed"], 0);
    assert_eq!(report["inactive_devices_removed"], 0);

    // Revocation puis oubli de la TV.
    let revoked = call(&app, "DELETE", &path, Some(&admin.access_token), None).await;

    assert_eq!(revoked.status(), StatusCode::NO_CONTENT);

    let forgotten = call(&app, "DELETE", &history, Some(&admin.access_token), None).await;

    assert_eq!(forgotten.status(), StatusCode::NO_CONTENT);

    let invalid_token = call(&app, "GET", "/v1/auth/me", Some(&tv.access_token), None).await;

    assert_eq!(invalid_token.status(), StatusCode::UNAUTHORIZED);

    let still_valid = call(&app, "GET", "/v1/auth/me", Some(&admin.access_token), None).await;

    assert_eq!(still_valid.status(), StatusCode::OK);
}
