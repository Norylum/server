use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use web_sys::window;

pub fn get_token() -> Option<String> {
    window()?
        .session_storage()
        .ok()??
        .get_item("norylum_token")
        .ok()?
}

pub fn set_token(token: &str) {
    if let Some(Ok(Some(storage))) = window().map(|w| w.session_storage()) {
        let _ = storage.set_item("norylum_token", token);
    }
}

pub fn clear_token() {
    if let Some(Ok(Some(storage))) = window().map(|w| w.session_storage()) {
        let _ = storage.remove_item("norylum_token");
    }
}

#[derive(Debug, Deserialize)]
pub struct LoginResponse {
    pub token: String,
}

#[derive(Debug, Serialize)]
struct LoginPayload<'a> {
    username: &'a str,
    password: &'a str,
}

pub async fn login(username: &str, password: &str) -> Result<String, String> {
    let resp = Request::post("/api/v1/auth/login")
        .json(&LoginPayload { username, password })
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if resp.status() == 200 {
        let body: LoginResponse = resp.json().await.map_err(|e| e.to_string())?;
        Ok(body.token)
    } else {
        Err(format!("Authentification échouée ({})", resp.status()))
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct MediaItem {
    pub id: String,
    pub title: String,
    pub media_type: Option<String>,
}

pub async fn list_media() -> Result<Vec<MediaItem>, String> {
    let token = get_token().ok_or("Non authentifié")?;
    let resp = Request::get("/api/v1/library/media")
        .header("Authorization", &format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    resp.json().await.map_err(|e| e.to_string())
}

#[derive(Debug, Deserialize, Clone)]
pub struct AcquisitionRequest {
    pub id: String,
    pub status: String,
    pub tmdb_id: u64,
    pub kind: String,
}

pub async fn list_acquisitions() -> Result<Vec<AcquisitionRequest>, String> {
    let token = get_token().ok_or("Non authentifié")?;
    let resp = Request::get("/api/v1/acquisition/requests")
        .header("Authorization", &format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    resp.json().await.map_err(|e| e.to_string())
}

pub async fn approve_acquisition(id: &str) -> Result<(), String> {
    let token = get_token().ok_or("Non authentifié")?;
    let resp = Request::post(&format!("/api/v1/acquisition/requests/{id}/approve"))
        .header("Authorization", &format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status() == 200 {
        Ok(())
    } else {
        Err(format!("Erreur ({})", resp.status()))
    }
}

pub async fn cancel_acquisition(id: &str) -> Result<(), String> {
    let token = get_token().ok_or("Non authentifié")?;
    let resp = Request::post(&format!("/api/v1/acquisition/requests/{id}/cancel"))
        .header("Authorization", &format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status() == 200 {
        Ok(())
    } else {
        Err(format!("Erreur ({})", resp.status()))
    }
}

#[derive(Debug, Serialize)]
pub struct RegisterSourcePayload<'a> {
    pub source_url: &'a str,
    pub expected_sha256: &'a str,
    pub allowed_hosts: Vec<&'a str>,
}

pub async fn register_source(id: &str, payload: &RegisterSourcePayload<'_>) -> Result<(), String> {
    let token = get_token().ok_or("Non authentifié")?;
    let resp = Request::post(&format!("/api/v1/acquisition/requests/{id}/source"))
        .header("Authorization", &format!("Bearer {token}"))
        .json(payload)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status() == 201 {
        Ok(())
    } else {
        Err(format!("Erreur ({})", resp.status()))
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct TorrentShare {
    pub media_id: String,
    pub enabled: bool,
    pub uploaded_bytes: u64,
    pub upload_bps: u64,
    pub peer_count: Option<u32>,
}

pub async fn list_shares() -> Result<Vec<TorrentShare>, String> {
    let token = get_token().ok_or("Non authentifié")?;
    let resp = Request::get("/api/v1/torrent/shares")
        .header("Authorization", &format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    resp.json().await.map_err(|e| e.to_string())
}
