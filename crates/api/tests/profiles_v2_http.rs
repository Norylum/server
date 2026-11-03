use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    response::Response,
    Router,
};
use norylum_api::{create_router, AppState};
use norylum_authorization::DefaultAuthorizer;
use norylum_catalog::CatalogService;
use norylum_foundation::ProfileId;
use norylum_playback::PlaybackService;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const PASSWORD: &str = "http-test-password-2026";

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
async fn test_profiles_v2_http_security() {
    let state = AppState::new(
        std::env::temp_dir(),
        Arc::new(CatalogService::in_memory()),
        Arc::new(DefaultAuthorizer::new()),
        Arc::new(PlaybackService::in_memory()),
    );

    let auth = state.auth.clone();

    auth.bootstrap_admin("admin", PASSWORD).await.unwrap();

    let admin = auth.login("admin", PASSWORD).await.unwrap();
    let app = create_router(state);

    let unauthenticated = call(
        &app,
        "PUT",
        "/v1/profiles/unknown",
        None,
        Some(json!({
            "name": "Test",
            "is_child": true,
            "age_rating_max": 10
        })),
    )
    .await;

    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let created = call(
        &app,
        "POST",
        "/v1/profiles",
        Some(&admin.access_token),
        Some(json!({
            "name": "Enfant",
            "is_child": true,
            "age_rating_max": 10
        })),
    )
    .await;

    assert_eq!(created.status(), StatusCode::CREATED);

    let child = json_body(created).await;
    let child_id = child["id"].as_str().unwrap().to_owned();

    let child_session = auth
        .switch_profile(&admin.access_token, &ProfileId::new(&child_id))
        .await
        .unwrap();

    let child_update = call(
        &app,
        "PUT",
        &format!("/v1/profiles/{child_id}"),
        Some(&child_session.access_token),
        Some(json!({
            "name": "Falsifie",
            "is_child": false,
            "age_rating_max": null
        })),
    )
    .await;

    assert_eq!(child_update.status(), StatusCode::FORBIDDEN);

    let updated = call(
        &app,
        "PUT",
        &format!("/v1/profiles/{child_id}"),
        Some(&admin.access_token),
        Some(json!({
            "name": "Adulte test",
            "is_child": false,
            "age_rating_max": null
        })),
    )
    .await;

    assert_eq!(updated.status(), StatusCode::OK);

    // La session enfant a ete revoquee.
    let revoked = call(
        &app,
        "GET",
        "/v1/auth/me",
        Some(&child_session.access_token),
        None,
    )
    .await;

    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);

    // Pas de passage vers un adulte sans mot de passe.
    let blocked = call(
        &app,
        "POST",
        "/v1/auth/switch-profile",
        Some(&admin.access_token),
        Some(json!({ "profile_id": child_id })),
    )
    .await;

    assert_eq!(blocked.status(), StatusCode::FORBIDDEN);

    let permitted = call(
        &app,
        "POST",
        "/v1/auth/switch-profile",
        Some(&admin.access_token),
        Some(json!({
            "profile_id": child_id,
            "account_password": PASSWORD
        })),
    )
    .await;

    assert_eq!(permitted.status(), StatusCode::OK);

    let adult_session = json_body(permitted).await;
    let adult_token = adult_session["access_token"].as_str().unwrap();

    let deleted = call(
        &app,
        "DELETE",
        &format!("/v1/profiles/{child_id}"),
        Some(&admin.access_token),
        None,
    )
    .await;

    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

    let revoked_adult = call(&app, "GET", "/v1/auth/me", Some(adult_token), None).await;

    assert_eq!(revoked_adult.status(), StatusCode::UNAUTHORIZED);
}
