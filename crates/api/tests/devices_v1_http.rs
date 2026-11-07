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

const PASSWORD: &str = "http-devices-password-2026";

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
async fn test_devices_v1_http_security() {
    let state = AppState::new(
        std::env::temp_dir(),
        Arc::new(CatalogService::in_memory()),
        Arc::new(DefaultAuthorizer::new()),
        Arc::new(PlaybackService::in_memory()),
            norylum_transcoding::HwAccel::Software,
    );

    let auth = state.auth.clone();

    auth.bootstrap_admin("admin", PASSWORD).await.unwrap();

    let app = create_router(state);

    let anonymous = call(&app, "GET", "/v1/devices", None, None).await;

    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let first = call(
        &app,
        "POST",
        "/v1/auth/login",
        None,
        Some(json!({
            "username": "admin",
            "password": PASSWORD,
            "device_name": "Mac"
        })),
    )
    .await;

    assert_eq!(first.status(), StatusCode::OK);
    let first = json_body(first).await;

    let second = call(
        &app,
        "POST",
        "/v1/auth/login",
        None,
        Some(json!({
            "username": "admin",
            "password": PASSWORD,
            "device_name": "TV salon"
        })),
    )
    .await;

    assert_eq!(second.status(), StatusCode::OK);
    let second = json_body(second).await;

    let token1 = first["access_token"].as_str().unwrap();
    let token2 = second["access_token"].as_str().unwrap();
    let device1 = first["device_id"].as_str().unwrap();
    let session2 = second["session_id"].as_str().unwrap();

    assert_ne!(first["device_id"], second["device_id"]);

    let list = call(&app, "GET", "/v1/devices", Some(token1), None).await;

    assert_eq!(list.status(), StatusCode::OK);

    let items = json_body(list).await;

    assert_eq!(items.as_array().unwrap().len(), 2);

    let sessions = call(&app, "GET", "/v1/devices/sessions", Some(token1), None).await;

    assert_eq!(sessions.status(), StatusCode::OK);

    let sessions = json_body(sessions).await;

    assert_eq!(sessions.as_array().unwrap().len(), 2);

    // Aucune session ne divulgue son jeton d'acces.
    let serialized = sessions.to_string();
    assert!(!serialized.contains(token1));
    assert!(!serialized.contains(token2));
    assert!(!serialized.contains("token_hash"));

    // Revoquer la session de la TV.
    let revoked = call(
        &app,
        "DELETE",
        &format!("/v1/devices/sessions/{session2}"),
        Some(token1),
        None,
    )
    .await;

    assert_eq!(revoked.status(), StatusCode::NO_CONTENT);

    let tv = call(&app, "GET", "/v1/auth/me", Some(token2), None).await;

    assert_eq!(tv.status(), StatusCode::UNAUTHORIZED);

    // Revoquer l'appareil courant.
    let revoked_device = call(
        &app,
        "DELETE",
        &format!("/v1/devices/{device1}"),
        Some(token1),
        None,
    )
    .await;

    assert_eq!(revoked_device.status(), StatusCode::NO_CONTENT);

    let old_session = call(&app, "GET", "/v1/auth/me", Some(token1), None).await;

    assert_eq!(old_session.status(), StatusCode::UNAUTHORIZED);
}
