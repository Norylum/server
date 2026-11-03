//! Routes HTTP des appareils et sessions.

use crate::auth_routes::bearer;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use norylum_devices::{DeviceRecord, DeviceService, MaintenanceReport, SessionRecord};
use norylum_foundation::{DeviceId, NorylumError};
use serde::Deserialize;
use std::sync::Arc;

pub(crate) async fn list_devices_handler(
    State(devices): State<Arc<DeviceService>>,
    headers: HeaderMap,
) -> Result<Json<Vec<DeviceRecord>>, NorylumError> {
    Ok(Json(devices.list_devices(bearer(&headers)?).await?))
}

pub(crate) async fn list_sessions_handler(
    State(devices): State<Arc<DeviceService>>,
    headers: HeaderMap,
) -> Result<Json<Vec<SessionRecord>>, NorylumError> {
    Ok(Json(devices.list_sessions(bearer(&headers)?).await?))
}

pub(crate) async fn revoke_device_handler(
    State(devices): State<Arc<DeviceService>>,
    headers: HeaderMap,
    Path(device_id): Path<DeviceId>,
) -> Result<StatusCode, NorylumError> {
    devices.revoke_device(bearer(&headers)?, &device_id).await?;

    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn revoke_session_handler(
    State(devices): State<Arc<DeviceService>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<StatusCode, NorylumError> {
    devices
        .revoke_session(bearer(&headers)?, &session_id)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct RenameDevicePayload {
    name: String,
}

/// PATCH /v1/devices/:device_id
pub(crate) async fn rename_device_handler(
    State(devices): State<Arc<DeviceService>>,
    headers: HeaderMap,
    Path(device_id): Path<DeviceId>,
    Json(payload): Json<RenameDevicePayload>,
) -> Result<StatusCode, NorylumError> {
    devices
        .rename_device(bearer(&headers)?, &device_id, &payload.name)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// POST /v1/devices/maintenance
pub(crate) async fn maintenance_handler(
    State(devices): State<Arc<DeviceService>>,
    headers: HeaderMap,
) -> Result<Json<MaintenanceReport>, NorylumError> {
    Ok(Json(devices.maintenance(bearer(&headers)?).await?))
}

/// DELETE /v1/devices/:device_id/history
pub(crate) async fn forget_device_handler(
    State(devices): State<Arc<DeviceService>>,
    headers: HeaderMap,
    Path(device_id): Path<DeviceId>,
) -> Result<StatusCode, NorylumError> {
    devices.forget_device(bearer(&headers)?, &device_id).await?;

    Ok(StatusCode::NO_CONTENT)
}
