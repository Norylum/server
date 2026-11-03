//! Routes HTTP et middleware d'authentification.

use axum::{
    extract::{Request, State},
    http::{header::AUTHORIZATION, HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::Response,
    Extension, Json,
};
use norylum_auth::{AuthService, LoginResponse};
use norylum_foundation::{NorylumError, ProfileId, RequestContext};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
pub(crate) struct LoginPayload {
    username: String,
    password: String,
}

#[derive(Deserialize)]
pub(crate) struct SwitchProfilePayload {
    profile_id: ProfileId,
}

fn bearer(headers: &HeaderMap) -> Result<&str, NorylumError> {
    headers
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|t| !t.is_empty() && !t.contains(char::is_whitespace))
        .ok_or_else(|| NorylumError::Unauthorized("Jeton Bearer requis".into()))
}

pub(crate) async fn auth_middleware(
    State(auth): State<Arc<AuthService>>,
    mut request: Request,
    next: Next,
) -> Result<Response, NorylumError> {
    let ctx = auth.authenticate(bearer(request.headers())?).await?;

    // Les en-tetes transmis par le client ne sont jamais fiables.
    // On les remplace par l'identite verifiee dans SQLite.
    let headers = request.headers_mut();

    for name in [
        "x-account-id",
        "x-profile-id",
        "x-device-id",
        "x-is-child",
        "x-age-rating-max",
    ] {
        headers.remove(name);
    }

    headers.insert(
        "x-account-id",
        HeaderValue::from_str(ctx.account_id().as_str())
            .map_err(|_| NorylumError::Internal("Identifiant compte invalide".into()))?,
    );

    headers.insert(
        "x-profile-id",
        HeaderValue::from_str(ctx.profile_id().as_str())
            .map_err(|_| NorylumError::Internal("Identifiant profil invalide".into()))?,
    );

    headers.insert("x-device-id", HeaderValue::from_static("dev_authenticated"));

    headers.insert(
        "x-is-child",
        HeaderValue::from_static(if ctx.is_child() { "true" } else { "false" }),
    );

    if let Some(age) = ctx.age_rating_max {
        headers.insert(
            "x-age-rating-max",
            HeaderValue::from_str(&age.to_string())
                .map_err(|_| NorylumError::Internal("Classification invalide".into()))?,
        );
    }

    request.extensions_mut().insert(ctx);
    Ok(next.run(request).await)
}

pub(crate) async fn login_handler(
    State(auth): State<Arc<AuthService>>,
    Json(payload): Json<LoginPayload>,
) -> Result<Json<LoginResponse>, NorylumError> {
    Ok(Json(
        auth.login(&payload.username, &payload.password).await?,
    ))
}

pub(crate) async fn logout_handler(
    State(auth): State<Arc<AuthService>>,
    headers: HeaderMap,
) -> Result<StatusCode, NorylumError> {
    auth.logout(bearer(&headers)?).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn me_handler(Extension(ctx): Extension<RequestContext>) -> Json<RequestContext> {
    Json(ctx)
}

pub(crate) async fn switch_profile_handler(
    State(auth): State<Arc<AuthService>>,
    headers: HeaderMap,
    Json(payload): Json<SwitchProfilePayload>,
) -> Result<Json<LoginResponse>, NorylumError> {
    Ok(Json(
        auth.switch_profile(bearer(&headers)?, &payload.profile_id)
            .await?,
    ))
}
