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

async fn request(
    app: &axum::Router,
    method: &str,
    route: &str,
    token: Option<&str>,
    data: Option<serde_json::Value>,
) -> StatusCode {
    let mut builder = Request::builder().method(method).uri(route);

    if let Some(token) = token {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }

    let body = if let Some(data) = data {
        builder = builder.header("Content-Type", "application/json");
        Body::from(data.to_string())
    } else {
        Body::empty()
    };

    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn torrent_control_is_principal_only() {
    let db = Database::in_memory().unwrap();

    let state = AppState::new(
        std::env::temp_dir(),
        Arc::new(CatalogService::new(db.clone())),
        Arc::new(DefaultAuthorizer::new()),
        Arc::new(PlaybackService::new(db)),
        norylum_transcoding::HwAccel::Software,
    );

    let auth = state.auth.clone();
    let password = "torrent-v2-test-password-2026";

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

    assert_eq!(
        request(&app, "GET", "/v1/torrent/shares", None, None,).await,
        StatusCode::UNAUTHORIZED,
    );

    assert_eq!(
        request(
            &app,
            "GET",
            "/v1/torrent/shares",
            Some(&child.access_token),
            None,
        )
        .await,
        StatusCode::FORBIDDEN,
    );

    assert_eq!(
        request(
            &app,
            "GET",
            "/v1/torrent/shares",
            Some(&adult.access_token),
            None,
        )
        .await,
        StatusCode::OK,
    );

    assert_eq!(
        request(
            &app,
            "PUT",
            "/v1/torrent/settings",
            Some(&child.access_token),
            Some(json!({"upload_kibps":128})),
        )
        .await,
        StatusCode::FORBIDDEN,
    );

    assert_eq!(
        request(
            &app,
            "PUT",
            "/v1/torrent/settings",
            Some(&adult.access_token),
            Some(json!({"upload_kibps":128})),
        )
        .await,
        StatusCode::OK,
    );

    assert_eq!(
        request(
            &app,
            "PUT",
            "/v1/torrent/settings",
            Some(&adult.access_token),
            Some(json!({"upload_kibps":999})),
        )
        .await,
        StatusCode::BAD_REQUEST,
    );

    let route = "/v1/torrent/media/med_1234567890abcdef/enable";

    assert_eq!(
        request(
            &app,
            "POST",
            route,
            Some(&adult.access_token),
            Some(json!({
                "confirm_distribution_rights":false
            })),
        )
        .await,
        StatusCode::BAD_REQUEST,
    );

    assert_eq!(
        request(
            &app,
            "POST",
            route,
            Some(&child.access_token),
            Some(json!({
                "confirm_distribution_rights":true
            })),
        )
        .await,
        StatusCode::FORBIDDEN,
    );

    assert_eq!(
        request(
            &app,
            "POST",
            route,
            Some(&adult.access_token),
            Some(json!({
                "confirm_distribution_rights":true
            })),
        )
        .await,
        StatusCode::NOT_FOUND,
    );
}
