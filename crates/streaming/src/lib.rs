//! Crate `streaming` pour Norylum Server.
//! Fournit le streaming vidéo "Direct Play" haute performance avec support complet
//! des requêtes HTTP 206 Partial Content et streaming par blocs bornés sans surcharge RAM.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::{
    HeaderMap, ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE,
};
use axum::http::StatusCode;
use axum::response::Response;
use norylum_authorization::{Authorizer, MediaAccessInfo};
use norylum_catalog::CatalogService;
use norylum_foundation::{AccountId, DeviceId, MediaId, NorylumError, ProfileId, RequestContext};
use norylum_library::PathSanitizer;
use std::io::SeekFrom;
use std::path::{Path as StdPath, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

/// Taille de chaque bloc de lecture tamponné (64 Ko) pour garantir une utilisation RAM strictement O(1).
pub const STREAM_BUFFER_SIZE: usize = 64 * 1024;

/// Configuration et dépendances requises pour le service de streaming.
#[derive(Clone)]
pub struct StreamingState {
    pub library_root: PathBuf,
    pub catalog: Arc<CatalogService>,
    pub authorizer: Arc<dyn Authorizer>,
}

/// Erreur survenue lors du parsing de l'en-tête HTTP Range.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RangeParseError {
    #[error("Format d'en-tête Range invalide")]
    InvalidFormat,
    #[error("Plage d'octets hors limites ou insatisfaisable")]
    Unsatisfiable,
}

/// Représente une plage d'octets demandée par le client.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

impl ByteRange {
    /// Longueur en octets de la plage
    pub fn length(&self) -> u64 {
        (self.end - self.start) + 1
    }

    /// Analyse l'en-tête HTTP `Range: bytes=...`
    pub fn parse(range_header: &str, total_size: u64) -> Result<Self, RangeParseError> {
        if total_size == 0 {
            return Err(RangeParseError::Unsatisfiable);
        }

        let range_str = range_header.trim();
        let bytes_prefix = "bytes=";
        if !range_str.starts_with(bytes_prefix) {
            return Err(RangeParseError::InvalidFormat);
        }

        let spec = &range_str[bytes_prefix.len()..];
        // Si plusieurs plages séparées par des virgules, nous prenons la première pour le streaming média
        let first_spec = spec.split(',').next().unwrap_or("").trim();

        if let Some(suffix) = first_spec.strip_prefix('-') {
            // Suffix range: bytes=-500 (derniers 500 octets)
            let suffix_len: u64 = suffix.parse().map_err(|_| RangeParseError::InvalidFormat)?;
            if suffix_len == 0 {
                return Err(RangeParseError::InvalidFormat);
            }
            let start = total_size.saturating_sub(suffix_len);
            let end = total_size - 1;
            Ok(ByteRange { start, end })
        } else if let Some((start_str, end_str)) = first_spec.split_once('-') {
            let start: u64 = start_str
                .parse()
                .map_err(|_| RangeParseError::InvalidFormat)?;
            if start >= total_size {
                return Err(RangeParseError::Unsatisfiable);
            }

            let end = if end_str.is_empty() {
                // Open range: bytes=1024-
                total_size - 1
            } else {
                // Closed range: bytes=1024-2048
                let parsed_end: u64 = end_str
                    .parse()
                    .map_err(|_| RangeParseError::InvalidFormat)?;
                parsed_end.min(total_size - 1)
            };

            if start > end {
                return Err(RangeParseError::Unsatisfiable);
            }

            Ok(ByteRange { start, end })
        } else {
            Err(RangeParseError::InvalidFormat)
        }
    }
}

/// Extrait le contexte de sécurité de la requête depuis les en-têtes HTTP (fail-closed par défaut).
pub fn extract_request_context_from_headers(headers: &HeaderMap) -> RequestContext {
    let account_id = headers
        .get("x-account-id")
        .and_then(|v| v.to_str().ok())
        .map(AccountId::new);

    let profile_id = headers
        .get("x-profile-id")
        .and_then(|v| v.to_str().ok())
        .map(ProfileId::new);

    let device_id = headers
        .get("x-device-id")
        .and_then(|v| v.to_str().ok())
        .map(DeviceId::new)
        .unwrap_or_else(|| DeviceId::new("web-direct-play"));

    let is_child = headers
        .get("x-is-child")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    let age_rating_max = headers
        .get("x-age-rating-max")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u32>().ok());

    match (account_id, profile_id) {
        (Some(acc), Some(prof)) => {
            RequestContext::authenticated(acc, prof, device_id, is_child, age_rating_max)
        }
        _ => RequestContext::unauthenticated(),
    }
}

/// Handler HTTP pour la route `/v1/media/:media_id`.
/// Gère le Direct Play 206 Partial Content et le streaming borné par chunks.
pub async fn stream_media_handler(
    State(state): State<StreamingState>,
    Path(media_id): Path<MediaId>,
    headers: HeaderMap,
) -> Result<Response, NorylumError> {
    // 1. Validation de contexte et sécurité fail-closed
    let ctx = extract_request_context_from_headers(&headers);
    state.authorizer.authorize_request(&ctx).await?;

    // 2. Recherche du média dans le catalogue
    let media = state
        .catalog
        .get_by_id(&media_id)
        .await
        .ok_or_else(|| NorylumError::NotFound(format!("Média '{}' introuvable", media_id)))?;

    // 3. Contrôle d'accès au média (droits profil, sécurité enfant)
    let access_info = MediaAccessInfo::new(media.id.clone(), media.is_child_safe, media.age_rating);
    state
        .authorizer
        .authorize_media_access(&ctx, &access_info)
        .await?;

    // 4. Vérification d'isolation physique anti-Directory Traversal
    let safe_file_path = PathSanitizer::sanitize_path(&state.library_root, &media.file_path)?;

    // 5. Exécution du streaming avec support 206 / 200
    serve_media_stream(&safe_file_path, &media.mime_type, headers.get(RANGE)).await
}

/// Sert un flux vidéo en streaming sans jamais charger le fichier entier en mémoire.
pub async fn serve_media_stream(
    file_path: &StdPath,
    mime_type: &str,
    range_header: Option<&axum::http::HeaderValue>,
) -> Result<Response, NorylumError> {
    let mut file = tokio::fs::File::open(file_path).await.map_err(|e| {
        NorylumError::NotFound(format!("Fichier média introuvable sur le disque : {}", e))
    })?;

    let metadata = file.metadata().await.map_err(NorylumError::Io)?;
    let total_size = metadata.len();

    if let Some(range_raw) = range_header.and_then(|v| v.to_str().ok()) {
        match ByteRange::parse(range_raw, total_size) {
            Ok(range) => {
                // Seek vers l'octet de début
                file.seek(SeekFrom::Start(range.start))
                    .await
                    .map_err(NorylumError::Io)?;

                let chunk_len = range.length();
                // Bounded stream : lit au maximum chunk_len octets par morceaux de STREAM_BUFFER_SIZE
                let limited_reader = file.take(chunk_len);
                let stream = ReaderStream::with_capacity(limited_reader, STREAM_BUFFER_SIZE);
                let body = Body::from_stream(stream);

                tracing::debug!(
                    start = range.start,
                    end = range.end,
                    total = total_size,
                    chunk_len,
                    "Envoi de réponse 206 Partial Content"
                );

                let response = Response::builder()
                    .status(StatusCode::PARTIAL_CONTENT)
                    .header(ACCEPT_RANGES, "bytes")
                    .header(
                        CONTENT_RANGE,
                        format!("bytes {}-{}/{}", range.start, range.end, total_size),
                    )
                    .header(CONTENT_LENGTH, chunk_len.to_string())
                    .header(CONTENT_TYPE, mime_type)
                    .body(body)
                    .map_err(|e| NorylumError::Internal(e.to_string()))?;

                Ok(response)
            }
            Err(_) => {
                // Range Invalide -> 416 Range Not Satisfiable
                tracing::warn!(
                    range = range_raw,
                    total_size,
                    "Plage d'octets invalide demandée par le client"
                );
                let response = Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(ACCEPT_RANGES, "bytes")
                    .header(CONTENT_RANGE, format!("bytes */{}", total_size))
                    .body(Body::empty())
                    .map_err(|e| NorylumError::Internal(e.to_string()))?;

                Ok(response)
            }
        }
    } else {
        // Pas d'en-tête Range -> Réponse 200 OK avec streaming intégral par morceaux
        let stream = ReaderStream::with_capacity(file, STREAM_BUFFER_SIZE);
        let body = Body::from_stream(stream);

        tracing::debug!(total = total_size, "Envoi de réponse 200 OK pour streaming");

        let response = Response::builder()
            .status(StatusCode::OK)
            .header(ACCEPT_RANGES, "bytes")
            .header(CONTENT_LENGTH, total_size.to_string())
            .header(CONTENT_TYPE, mime_type)
            .body(body)
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use norylum_authorization::DefaultAuthorizer;
    use norylum_library::MediaItem;
    use std::io::Write;

    #[test]
    fn test_byte_range_parsing() {
        let total = 1000;

        // bytes=0-499
        let r = ByteRange::parse("bytes=0-499", total).unwrap();
        assert_eq!(r.start, 0);
        assert_eq!(r.end, 499);
        assert_eq!(r.length(), 500);

        // bytes=500- (jusqu'à la fin)
        let r = ByteRange::parse("bytes=500-", total).unwrap();
        assert_eq!(r.start, 500);
        assert_eq!(r.end, 999);
        assert_eq!(r.length(), 500);

        // bytes=-200 (les 200 derniers octets)
        let r = ByteRange::parse("bytes=-200", total).unwrap();
        assert_eq!(r.start, 800);
        assert_eq!(r.end, 999);
        assert_eq!(r.length(), 200);

        // Clamping si end > total
        let r = ByteRange::parse("bytes=950-2000", total).unwrap();
        assert_eq!(r.start, 950);
        assert_eq!(r.end, 999);
        assert_eq!(r.length(), 50);

        // Invalide si start >= total
        assert!(ByteRange::parse("bytes=1000-", total).is_err());
        assert!(ByteRange::parse("bytes=1500-2000", total).is_err());
        assert!(ByteRange::parse("bytes=500-400", total).is_err());
        assert!(ByteRange::parse("invalid", total).is_err());
    }

    #[tokio::test]
    async fn test_serve_media_stream_full_and_range() {
        let temp_dir =
            std::env::temp_dir().join(format!("norylum_stream_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let file_path = temp_dir.join("test_video.mp4");
        let content: Vec<u8> = (0..200).collect();
        {
            let mut f = std::fs::File::create(&file_path).unwrap();
            f.write_all(&content).unwrap();
        }

        // Test 1: Pas de Range -> 200 OK
        let resp200 = serve_media_stream(&file_path, "video/mp4", None)
            .await
            .unwrap();
        assert_eq!(resp200.status(), StatusCode::OK);
        assert_eq!(resp200.headers().get(ACCEPT_RANGES).unwrap(), "bytes");
        assert_eq!(resp200.headers().get(CONTENT_LENGTH).unwrap(), "200");

        // Test 2: Range 10-29 -> 206 Partial Content
        let range_val = HeaderValue::from_static("bytes=10-29");
        let resp206 = serve_media_stream(&file_path, "video/mp4", Some(&range_val))
            .await
            .unwrap();
        assert_eq!(resp206.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            resp206.headers().get(CONTENT_RANGE).unwrap(),
            "bytes 10-29/200"
        );
        assert_eq!(resp206.headers().get(CONTENT_LENGTH).unwrap(), "20");

        // Test 3: Range non satisfiable -> 416
        let bad_range = HeaderValue::from_static("bytes=500-600");
        let resp416 = serve_media_stream(&file_path, "video/mp4", Some(&bad_range))
            .await
            .unwrap();
        assert_eq!(resp416.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(resp416.headers().get(CONTENT_RANGE).unwrap(), "bytes */200");

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn test_stream_fail_closed_authorization() {
        let temp_dir =
            std::env::temp_dir().join(format!("norylum_stream_auth_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let video_path = temp_dir.join("secret_adult.mp4");
        std::fs::write(&video_path, b"adult content").unwrap();

        let catalog = Arc::new(CatalogService::new());
        let authorizer = Arc::new(DefaultAuthorizer::new());

        let media_id = MediaId::new("med_adult");
        catalog
            .add_item(MediaItem {
                id: media_id.clone(),
                title: "Adult Movie".into(),
                file_path: video_path.clone(),
                relative_path: "secret_adult.mp4".into(),
                file_size: 13,
                duration_seconds: 120.0,
                mime_type: "video/mp4".into(),
                video_codec: None,
                audio_codec: None,
                width: None,
                height: None,
                is_child_safe: false,
                age_rating: Some(18),
            })
            .await;

        let state = StreamingState {
            library_root: temp_dir.clone(),
            catalog,
            authorizer,
        };

        // Cas 1 : Requête non authentifiée -> 401
        let unauth_headers = HeaderMap::new();
        let res =
            stream_media_handler(State(state.clone()), Path(media_id.clone()), unauth_headers)
                .await;
        assert!(matches!(res, Err(NorylumError::Unauthorized(_))));

        // Cas 2 : Profil enfant tentant de lire un contenu adulte -> 403 Forbidden
        let mut child_headers = HeaderMap::new();
        child_headers.insert("x-account-id", HeaderValue::from_static("acc_family"));
        child_headers.insert("x-profile-id", HeaderValue::from_static("prof_kid"));
        child_headers.insert("x-is-child", HeaderValue::from_static("true"));
        let res =
            stream_media_handler(State(state.clone()), Path(media_id.clone()), child_headers).await;
        assert!(matches!(res, Err(NorylumError::Forbidden(_))));

        // Cas 3 : Profil adulte -> 200 OK
        let mut adult_headers = HeaderMap::new();
        adult_headers.insert("x-account-id", HeaderValue::from_static("acc_family"));
        adult_headers.insert("x-profile-id", HeaderValue::from_static("prof_dad"));
        let res = stream_media_handler(State(state.clone()), Path(media_id), adult_headers).await;
        assert!(res.is_ok());

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
