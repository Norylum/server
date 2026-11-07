//! Cache local des affiches TMDB.
//!
//! Télécharge les images depuis TMDB au moment où une association est posée,
//! et les sert depuis le volume local pour la résilience hors-ligne.

use std::path::{Path, PathBuf};

const TMDB_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/w500/";

/// Cache local des affiches TMDB, stockées dans `images_dir`.
#[derive(Clone)]
pub struct PosterCache {
    images_dir: PathBuf,
    client: reqwest::Client,
}

impl PosterCache {
    /// Crée le cache. `images_dir` est créé s'il n'existe pas.
    pub fn new(images_dir: PathBuf) -> Self {
        Self {
            images_dir,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
        }
    }

    /// Prépare le répertoire sur le disque. Appelé au démarrage.
    pub fn prepare_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.images_dir)
    }

    /// Télécharge et met en cache l'affiche pour `poster_path` (ex: `/abc123.jpg`).
    /// Retourne l'URL locale `/v1/images/posters/{filename}` ou `None` si échec.
    /// Idempotent : ne re-télécharge pas si le fichier existe déjà.
    pub async fn store(&self, poster_path: &str) -> Option<String> {
        let filename = safe_filename(poster_path)?;
        let dest = self.images_dir.join(&filename);

        if dest.exists() {
            return Some(local_url(&filename));
        }

        let url = format!("{TMDB_IMAGE_BASE}{filename}");

        let bytes = self
            .client
            .get(&url)
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .bytes()
            .await
            .ok()?;

        tokio::fs::write(&dest, &bytes).await.ok()?;

        Some(local_url(&filename))
    }

    /// Retourne l'URL locale si le fichier est présent en cache, sinon `None`.
    /// Extrait le nom de fichier depuis une URL TMDB complète.
    pub fn local_url_if_cached(&self, tmdb_url: &str) -> Option<String> {
        let filename = tmdb_url.split('/').next_back()?;
        if !is_safe_image_filename(filename) {
            return None;
        }
        if self.images_dir.join(filename).exists() {
            Some(local_url(filename))
        } else {
            None
        }
    }

    /// Chemin absolu vers un fichier mis en cache, pour le servir.
    pub fn cached_path(&self, filename: &str) -> Option<PathBuf> {
        if !is_safe_image_filename(filename) {
            return None;
        }
        let path = self.images_dir.join(filename);
        if path.exists() {
            Some(path)
        } else {
            None
        }
    }
}

fn safe_filename(poster_path: &str) -> Option<String> {
    // poster_path ressemble à "/abc123.jpg"
    let raw = Path::new(poster_path)
        .file_name()?
        .to_str()?;
    if is_safe_image_filename(raw) {
        Some(raw.to_owned())
    } else {
        None
    }
}

fn is_safe_image_filename(name: &str) -> bool {
    if name.is_empty() || name.len() > 128 {
        return false;
    }
    // Pas de séparateur de chemin, pas de point en début.
    if name.contains('/') || name.contains('\\') || name.starts_with('.') {
        return false;
    }
    // Extension image uniquement.
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".jpg") || lower.ends_with(".jpeg") || lower.ends_with(".png")
}

fn local_url(filename: &str) -> String {
    format!("/v1/images/posters/{filename}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_filename_extracts_and_validates() {
        assert_eq!(
            safe_filename("/abc123.jpg"),
            Some("abc123.jpg".to_owned())
        );
        assert!(safe_filename("/../etc/passwd").is_none());
        assert!(safe_filename("/safe.exe").is_none());
        assert!(safe_filename("").is_none());
    }

    #[test]
    fn local_url_if_cached_returns_none_for_missing_file() {
        let cache = PosterCache::new(PathBuf::from("/nonexistent/path"));
        assert!(cache
            .local_url_if_cached("https://image.tmdb.org/t/p/w500/abc.jpg")
            .is_none());
    }
}
