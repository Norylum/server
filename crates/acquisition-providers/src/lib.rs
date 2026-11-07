//! Fournisseurs d'acquisition interchangeables.
//!
//! Un `Provider` traduit une requête (tmdb_id + qualité + langue) en
//! résultats téléchargeables (HTTPS ou BitTorrent).

pub mod prowlarr;
pub use prowlarr::ProwlarrProvider;

use std::future::Future;

/// Type de source retournée par un fournisseur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceType {
    Https,
    Torrent,
}

/// Paramètres d'une recherche auprès d'un fournisseur.
#[derive(Debug, Clone)]
pub struct ProviderQuery {
    pub tmdb_id: u64,
    pub kind: String,
    pub quality_profile: String,
    pub language: String,
}

/// Un résultat de recherche proposé par un fournisseur.
#[derive(Debug, Clone)]
pub struct ProviderResult {
    pub title: String,
    pub source_type: SourceType,
    /// URL HTTPS ou magnet URI.
    pub url: String,
    /// Obligatoire pour HTTPS, absent pour BitTorrent.
    pub expected_sha256: Option<String>,
    pub seeders: Option<u32>,
    pub size_bytes: Option<u64>,
}

/// Interface commune à tous les fournisseurs d'acquisition.
pub trait Provider: Send + Sync {
    fn search(
        &self,
        query: ProviderQuery,
    ) -> impl Future<Output = Vec<ProviderResult>> + Send;
}
