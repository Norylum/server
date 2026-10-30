//! Crate `foundation` pour Norylum Server.
//! Déclare les identifiants typés, la gestion d'erreurs centrale et le contexte de requête.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

macro_rules! impl_id_type {
    ($name:ident, $prefix:expr) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Crée un nouvel identifiant à partir d'une chaîne
            pub fn new(val: impl Into<String>) -> Self {
                Self(val.into())
            }

            /// Génère un nouvel identifiant aléatoire (UUIDv4)
            pub fn random() -> Self {
                Self(format!("{}_{}", $prefix, Uuid::new_v4().simple()))
            }

            /// Accède à la tranche str sous-jacente
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }

        impl FromStr for $name {
            type Err = std::convert::Infallible;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self::from(s))
            }
        }

        impl std::ops::Deref for $name {
            type Target = str;
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
    };
}

impl_id_type!(MediaId, "med");
impl_id_type!(ProfileId, "prof");
impl_id_type!(AccountId, "acc");
impl_id_type!(DeviceId, "dev");
impl_id_type!(ShowId, "shw");
impl_id_type!(SeasonId, "sea");
impl_id_type!(EpisodeId, "epi");

impl ShowId {
    /// Génère un ShowId déterministe et stable à partir du chemin relatif normalisé du dossier de la série
    pub fn from_path(relative_path: &str) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"show:");
        hasher.update(
            relative_path
                .replace('\\', "/")
                .trim_matches('/')
                .to_lowercase()
                .as_bytes(),
        );
        let hex = format!("{:x}", hasher.finalize());
        Self::new(format!("shw_{}", &hex[..16]))
    }
}

impl SeasonId {
    /// Génère un SeasonId déterministe et stable à partir du ShowId et du numéro de saison
    pub fn from_show_and_season(show_id: &ShowId, season_number: u32) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"season:");
        hasher.update(show_id.as_str().as_bytes());
        hasher.update(season_number.to_be_bytes());
        let hex = format!("{:x}", hasher.finalize());
        Self::new(format!("sea_{}", &hex[..16]))
    }

    /// Génère un SeasonId déterministe et stable à partir du chemin relatif normalisé du dossier de saison
    pub fn from_path(relative_path: &str) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"season_path:");
        hasher.update(
            relative_path
                .replace('\\', "/")
                .trim_matches('/')
                .to_lowercase()
                .as_bytes(),
        );
        let hex = format!("{:x}", hasher.finalize());
        Self::new(format!("sea_{}", &hex[..16]))
    }
}

impl EpisodeId {
    /// Génère un EpisodeId déterministe et stable à partir du SeasonId et du numéro d'épisode
    pub fn from_season_and_episode(season_id: &SeasonId, episode_number: u32) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"episode:");
        hasher.update(season_id.as_str().as_bytes());
        hasher.update(episode_number.to_be_bytes());
        let hex = format!("{:x}", hasher.finalize());
        Self::new(format!("epi_{}", &hex[..16]))
    }

    /// Génère un EpisodeId déterministe et stable à partir du chemin relatif normalisé du fichier de l'épisode
    pub fn from_path(relative_path: &str) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"episode_path:");
        hasher.update(
            relative_path
                .replace('\\', "/")
                .trim_matches('/')
                .to_lowercase()
                .as_bytes(),
        );
        let hex = format!("{:x}", hasher.finalize());
        Self::new(format!("epi_{}", &hex[..16]))
    }
}

/// Erreurs standardisées du monolithe Norylum
#[derive(Debug, thiserror::Error)]
pub enum NorylumError {
    #[error("Non authentifié : {0}")]
    Unauthorized(String),

    #[error("Accès interdit : {0}")]
    Forbidden(String),

    #[error("Ressource non trouvée : {0}")]
    NotFound(String),

    #[error("Erreur de validation : {0}")]
    Validation(String),

    #[error("Tentative d'évasion de répertoire détectée : {0}")]
    DirectoryTraversal(String),

    #[error("Erreur lors de l'inspection ffprobe : {0}")]
    FfprobeError(String),

    #[error("Plage d'octets invalide (Range Not Satisfiable) : {0}")]
    RangeNotSatisfiable(String),

    #[error("Erreur d'entrée/sortie : {0}")]
    Io(#[from] std::io::Error),

    #[error("Erreur interne : {0}")]
    Internal(String),
}

impl NorylumError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) | Self::DirectoryTraversal(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Validation(_) => StatusCode::BAD_REQUEST,
            Self::RangeNotSatisfiable(_) => StatusCode::RANGE_NOT_SATISFIABLE,
            Self::FfprobeError(_) | Self::Internal(_) | Self::Io(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }

    pub fn error_code(&self) -> &'static str {
        match self {
            Self::Unauthorized(_) => "UNAUTHORIZED",
            Self::Forbidden(_) => "FORBIDDEN",
            Self::DirectoryTraversal(_) => "DIRECTORY_TRAVERSAL",
            Self::NotFound(_) => "NOT_FOUND",
            Self::Validation(_) => "VALIDATION_ERROR",
            Self::RangeNotSatisfiable(_) => "RANGE_NOT_SATISFIABLE",
            Self::FfprobeError(_) => "FFPROBE_ERROR",
            Self::Io(_) => "IO_ERROR",
            Self::Internal(_) => "INTERNAL_SERVER_ERROR",
        }
    }
}

impl IntoResponse for NorylumError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let body = axum::Json(serde_json::json!({
            "error": self.to_string(),
            "code": self.error_code(),
        }));
        (status, body).into_response()
    }
}

/// Contexte d'une requête HTTP traversant le monolithe modulaire.
/// Utilisé pour la politique de sécurité fail-closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestContext {
    pub account_id: AccountId,
    pub profile_id: ProfileId,
    pub device_id: DeviceId,
    pub is_child: bool,
    pub is_authenticated: bool,
    pub age_rating_max: Option<u32>,
}

impl RequestContext {
    /// Crée un contexte authentifié
    pub fn authenticated(
        account_id: impl Into<AccountId>,
        profile_id: impl Into<ProfileId>,
        device_id: impl Into<DeviceId>,
        is_child: bool,
        age_rating_max: Option<u32>,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            profile_id: profile_id.into(),
            device_id: device_id.into(),
            is_child,
            is_authenticated: true,
            age_rating_max,
        }
    }

    /// Crée un contexte non authentifié (fail-closed par défaut)
    pub fn unauthenticated() -> Self {
        Self {
            account_id: AccountId::new("anonymous"),
            profile_id: ProfileId::new("anonymous"),
            device_id: DeviceId::new("unknown"),
            is_child: false,
            is_authenticated: false,
            age_rating_max: None,
        }
    }

    pub fn is_authenticated(&self) -> bool {
        self.is_authenticated
    }

    pub fn is_child(&self) -> bool {
        self.is_child
    }

    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    pub fn profile_id(&self) -> &ProfileId {
        &self.profile_id
    }

    pub fn device_id(&self) -> &DeviceId {
        &self.device_id
    }
}

impl Default for RequestContext {
    fn default() -> Self {
        Self::unauthenticated()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_id_types() {
        let media_id = MediaId::new("med_12345");
        assert_eq!(media_id.as_str(), "med_12345");
        assert_eq!(media_id.to_string(), "med_12345");

        let rnd = MediaId::random();
        assert!(rnd.as_str().starts_with("med_"));

        let show_id = ShowId::from_path("Bécé Bégé");
        assert!(show_id.as_str().starts_with("shw_"));
        assert_eq!(show_id, ShowId::from_path("Bécé Bégé"));
        assert_ne!(show_id, ShowId::from_path("Other Show"));

        let season_id = SeasonId::from_show_and_season(&show_id, 1);
        assert!(season_id.as_str().starts_with("sea_"));
        assert_eq!(season_id, SeasonId::from_show_and_season(&show_id, 1));
        assert_ne!(season_id, SeasonId::from_show_and_season(&show_id, 2));

        let episode_id = EpisodeId::from_season_and_episode(&season_id, 3);
        assert!(episode_id.as_str().starts_with("epi_"));
        assert_eq!(
            episode_id,
            EpisodeId::from_season_and_episode(&season_id, 3)
        );
    }

    #[test]
    fn test_request_context_defaults() {
        let ctx = RequestContext::default();
        assert!(!ctx.is_authenticated());

        let auth_ctx = RequestContext::authenticated("acc_1", "prof_1", "dev_1", true, Some(10));
        assert!(auth_ctx.is_authenticated());
        assert!(auth_ctx.is_child());
        assert_eq!(auth_ctx.profile_id().as_str(), "prof_1");
    }

    #[test]
    fn test_error_status_codes() {
        let err = NorylumError::Forbidden("interdit".into());
        assert_eq!(err.status_code(), StatusCode::FORBIDDEN);

        let err = NorylumError::RangeNotSatisfiable("range".into());
        assert_eq!(err.status_code(), StatusCode::RANGE_NOT_SATISFIABLE);
    }
}
