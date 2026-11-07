//! Fournisseur Prowlarr : agrégateur d'indexeurs BitTorrent/Usenet.
//!
//! Variables d'environnement requises :
//!   NORYLUM_PROWLARR_URL  — ex: http://prowlarr:9696
//!   NORYLUM_PROWLARR_KEY  — clé API Prowlarr

use crate::{Provider, ProviderQuery, ProviderResult, SourceType};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

/// Résultat brut de l'API Prowlarr `/api/v1/search`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProwlarrRelease {
    title: Option<String>,
    magnet_url: Option<String>,
    download_url: Option<String>,
    seeders: Option<u32>,
    size: Option<u64>,
}

pub struct ProwlarrProvider {
    base_url: String,
    api_key: String,
    client: Client,
}

impl ProwlarrProvider {
    pub fn new(base_url: String, api_key: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key,
            client: Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
        }
    }

    /// Construit à partir des variables d'environnement. Retourne `None` si
    /// les variables sont absentes (Prowlarr non configuré).
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("NORYLUM_PROWLARR_URL").ok()?;
        let key = std::env::var("NORYLUM_PROWLARR_KEY").ok()?;
        if url.is_empty() || key.is_empty() {
            return None;
        }
        Some(Self::new(url, key))
    }

    async fn fetch(&self, query: &ProviderQuery) -> Vec<ProwlarrRelease> {
        // Prowlarr catégorie 2000 = Films, 5000 = TV
        let categories = if query.kind == "movie" { "2000" } else { "5000" };

        let search_query = format!(
            "tmdb:{}",
            query.tmdb_id
        );

        let url = format!("{}/api/v1/search", self.base_url);

        let result = self
            .client
            .get(&url)
            .header("X-Api-Key", &self.api_key)
            .query(&[
                ("query", search_query.as_str()),
                ("categories", categories),
                ("limit", "50"),
            ])
            .send()
            .await;

        match result {
            Ok(resp) if resp.status().is_success() => {
                resp.json::<Vec<ProwlarrRelease>>().await.unwrap_or_default()
            }
            Ok(resp) => {
                tracing_log(format!("Prowlarr HTTP {}", resp.status()));
                Vec::new()
            }
            Err(e) => {
                tracing_log(format!("Prowlarr inaccessible : {e}"));
                Vec::new()
            }
        }
    }
}

fn tracing_log(msg: String) {
    // Évite une dépendance tracing dans la crate provider.
    eprintln!("[acquisition-providers] {msg}");
}

impl Provider for ProwlarrProvider {
    async fn search(&self, query: ProviderQuery) -> Vec<ProviderResult> {
        let releases = self.fetch(&query).await;

        releases
            .into_iter()
            .filter_map(|r| {
                let title = r.title.unwrap_or_default();

                if let Some(magnet) = r.magnet_url.filter(|m| m.starts_with("magnet:")) {
                    return Some(ProviderResult {
                        title,
                        source_type: SourceType::Torrent,
                        url: magnet,
                        expected_sha256: None,
                        seeders: r.seeders,
                        size_bytes: r.size,
                    });
                }

                if let Some(dl_url) = r
                    .download_url
                    .filter(|u| u.starts_with("https://"))
                {
                    // Les releases Prowlarr HTTPS n'ont pas de SHA-256 ;
                    // elles seront enregistrées comme source torrent (URL .torrent).
                    return Some(ProviderResult {
                        title,
                        source_type: SourceType::Torrent,
                        url: dl_url,
                        expected_sha256: None,
                        seeders: r.seeders,
                        size_bytes: r.size,
                    });
                }

                None
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_env_returns_none_without_vars() {
        // Sans les variables d'environnement, pas de provider.
        // Ce test ne pollue pas l'env réel.
        unsafe {
            std::env::remove_var("NORYLUM_PROWLARR_URL");
            std::env::remove_var("NORYLUM_PROWLARR_KEY");
        }
        assert!(ProwlarrProvider::from_env().is_none());
    }
}
