//! Client TMDB : recherche et consultation explicite.
//! Aucun jeton ni identifiant de serveur dans les metadonnees.

use norylum_foundation::NorylumError;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::time::Duration;

const TMDB_API: &str = "https://api.themoviedb.org/3";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TmdbKind {
    Movie,
    Tv,
}

impl TmdbKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Tv => "tv",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TmdbItem {
    pub tmdb_id: u64,
    pub tmdb_ref: String,
    pub kind: TmdbKind,
    pub title: String,
    pub overview: String,
    pub poster_path: Option<String>,
    pub poster_url: Option<String>,
    pub release_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawItem {
    id: u64,
    title: Option<String>,
    name: Option<String>,
    overview: Option<String>,
    poster_path: Option<String>,
    release_date: Option<String>,
    first_air_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    results: Vec<RawItem>,
}

/// Ne construit une URL que depuis un chemin de poster
/// simple et sur le domaine officiel TMDB.
pub fn poster_url(path: &str) -> Option<String> {
    let filename = path.strip_prefix('/')?;

    if filename.is_empty()
        || filename.len() > 128
        || filename.contains("..")
        || !filename
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
    {
        return None;
    }

    Some(format!("https://image.tmdb.org/t/p/w500/{filename}"))
}

impl RawItem {
    fn normalized(self, kind: TmdbKind) -> Option<TmdbItem> {
        if self.id == 0 || self.id > i64::MAX as u64 {
            return None;
        }

        let title = self
            .title
            .or(self.name)?
            .trim()
            .chars()
            .take(300)
            .collect::<String>();

        if title.is_empty() {
            return None;
        }

        let poster_path = self.poster_path.filter(|path| poster_url(path).is_some());

        let poster_link = poster_path.as_deref().and_then(poster_url);

        Some(TmdbItem {
            tmdb_id: self.id,
            tmdb_ref: format!("{}/{}", kind.as_str(), self.id),
            kind,
            title,
            overview: self
                .overview
                .unwrap_or_default()
                .chars()
                .take(8000)
                .collect(),
            poster_path,
            poster_url: poster_link,
            release_date: self
                .release_date
                .or(self.first_air_date)
                .filter(|date| date.len() <= 32),
        })
    }
}

#[derive(Clone)]
pub struct TmdbClient {
    client: reqwest::Client,
    token: Option<String>,
}

impl TmdbClient {
    pub fn from_env() -> Self {
        let token = std::env::var("NORYLUM_TMDB_READ_TOKEN")
            .ok()
            .filter(|s| !s.trim().is_empty());

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("Configuration client HTTPS TMDB");

        Self { client, token }
    }

    async fn request<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, NorylumError> {
        let token = self
            .token
            .as_deref()
            .ok_or_else(|| NorylumError::Internal("Configurer NORYLUM_TMDB_READ_TOKEN".into()))?;

        // Les chemins sont construits uniquement par
        // les methodes internes, jamais depuis une URL client.
        let url = format!("{TMDB_API}{path}");

        let response = self
            .client
            .get(url)
            .bearer_auth(token)
            .query(query)
            .send()
            .await
            .map_err(|_| NorylumError::Internal("Connexion TMDB indisponible".into()))?;

        let status = response.status();

        if status.as_u16() == 404 {
            return Err(NorylumError::NotFound("Reference TMDB introuvable".into()));
        }

        if !status.is_success() {
            return Err(NorylumError::Internal(format!(
                "TMDB a retourne HTTP {}",
                status.as_u16()
            )));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|_| NorylumError::Internal("Reponse TMDB illisible".into()))?;

        if bytes.len() > 1_048_576 {
            return Err(NorylumError::Internal(
                "Reponse TMDB trop volumineuse".into(),
            ));
        }

        serde_json::from_slice(&bytes)
            .map_err(|_| NorylumError::Internal("Format de reponse TMDB invalide".into()))
    }

    /// Recherche de propositions, sans association automatique.
    pub async fn search(&self, kind: TmdbKind, query: &str) -> Result<Vec<TmdbItem>, NorylumError> {
        let query = query.trim();

        if query.len() < 2 || query.len() > 120 || query.chars().any(char::is_control) {
            return Err(NorylumError::Validation(
                "Recherche TMDB : 2 a 120 octets requis".into(),
            ));
        }

        let path = format!("/search/{}", kind.as_str());

        let response: SearchResponse = self
            .request(
                &path,
                &[
                    ("query", query),
                    ("language", "fr-FR"),
                    ("include_adult", "false"),
                    ("page", "1"),
                ],
            )
            .await?;

        Ok(response
            .results
            .into_iter()
            .filter_map(|raw| raw.normalized(kind))
            .take(10)
            .collect())
    }

    /// Recupere le detail exact du TMDB ID choisi.
    pub async fn details(&self, kind: TmdbKind, tmdb_id: u64) -> Result<TmdbItem, NorylumError> {
        if tmdb_id == 0 || tmdb_id > i64::MAX as u64 {
            return Err(NorylumError::Validation("Identifiant TMDB invalide".into()));
        }

        let path = format!("/{}/{tmdb_id}", kind.as_str());

        let raw: RawItem = self.request(&path, &[("language", "fr-FR")]).await?;

        if raw.id != tmdb_id {
            return Err(NorylumError::Internal("Reponse TMDB incoherente".into()));
        }

        raw.normalized(kind)
            .ok_or_else(|| NorylumError::Internal("Fiche TMDB incomplete".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_movie_and_tv_normalization() {
        let movie: RawItem = serde_json::from_str(
            r#"{
                "id": 603,
                "title": "The Matrix",
                "overview": "Synopsis",
                "poster_path": "/poster123.jpg",
                "release_date": "1999-03-31"
            }"#,
        )
        .unwrap();

        let result = movie.normalized(TmdbKind::Movie).unwrap();

        assert_eq!(result.tmdb_ref, "movie/603");
        assert_eq!(
            result.poster_url.as_deref(),
            Some("https://image.tmdb.org/t/p/w500/poster123.jpg")
        );

        let tv: RawItem = serde_json::from_str(
            r#"{
                "id": 1396,
                "name": "Breaking Bad",
                "first_air_date": "2008-01-20"
            }"#,
        )
        .unwrap();

        let result = tv.normalized(TmdbKind::Tv).unwrap();

        assert_eq!(result.tmdb_ref, "tv/1396");
        assert_eq!(result.release_date.as_deref(), Some("2008-01-20"));
    }

    #[test]
    fn test_poster_path_is_not_an_arbitrary_url() {
        assert!(poster_url("/safe.jpg").is_some());
        assert!(poster_url("/../../private.jpg").is_none());
        assert!(poster_url("https://example.org/a.jpg").is_none());
    }
}
