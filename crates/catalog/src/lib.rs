//! Crate `catalog` pour Norylum Server.
//! Maintient l'inventaire persistant des médias, films et séries indexés dans SQLite
//! et fournit des fonctions de recherche filtrées par les politiques d'autorisation.

use norylum_authorization::{Authorizer, MediaAccessInfo};
use norylum_foundation::{MediaId, NorylumError, RequestContext, ShowId};
use norylum_library::MediaItem;
use norylum_persistence::Database;
pub use norylum_persistence::{EpisodeRecord, PlaybackProgressRecord, SeasonRecord, ShowRecord};
use serde::{Deserialize, Serialize};

/// Détails complets d'une série avec la liste de ses saisons.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShowDetails {
    #[serde(flatten)]
    pub show: ShowRecord,
    pub seasons: Vec<SeasonRecord>,
}

/// Détails d'une saison avec la liste de ses épisodes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SeasonDetails {
    #[serde(flatten)]
    pub season: SeasonRecord,
    pub episodes: Vec<EpisodeRecord>,
}

/// Service central de catalogue média pour Norylum, adossé à SQLite local-first.
#[derive(Clone)]
pub struct CatalogService {
    db: Database,
}

impl CatalogService {
    /// Initialise le service de catalogue avec une instance de base de données SQLite.
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Crée une instance avec une base SQLite en mémoire vive (pour tests).
    pub fn in_memory() -> Self {
        let db = Database::in_memory().expect("Échec d'initialisation SQLite en mémoire");
        Self { db }
    }

    /// Accède à la référence de la base de données sous-jacente.
    pub fn db(&self) -> &Database {
        &self.db
    }

    /// Ajoute ou met à jour un média dans le catalogue persistant.
    pub async fn add_item(&self, item: MediaItem) {
        if let Err(e) = self.db.sync_media_items(&[item]).await {
            tracing::error!(error = %e, "Échec lors de l'enregistrement SQLite du média");
        }
    }

    /// Synchronise l'ensemble des éléments indexés dans SQLite.
    /// Préserve les IDs stables existants et supprime les éléments orphelins.
    pub async fn replace_all(&self, new_items: Vec<MediaItem>) -> Vec<MediaItem> {
        match self.db.sync_media_items(&new_items).await {
            Ok(items) => items,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "Échec de la synchronisation SQLite du catalogue"
                );
                Vec::new()
            }
        }
    }

    /// Récupère un média par son identifiant unique depuis SQLite.
    pub async fn get_by_id(&self, id: &MediaId) -> Option<MediaItem> {
        self.db.get_media_by_id(id).await.unwrap_or(None)
    }

    /// Liste l'ensemble des médias du catalogue (sans contrôle de contexte).
    pub async fn list_all(&self) -> Vec<MediaItem> {
        self.db.list_all_media().await.unwrap_or_default()
    }

    /// Liste uniquement les médias accessibles selon le contexte de requête et les règles fail-closed de l'Authorizer.
    pub async fn list_for_context(
        &self,
        ctx: &RequestContext,
        authorizer: &dyn Authorizer,
    ) -> Vec<MediaItem> {
        if authorizer.authorize_request(ctx).await.is_err() {
            return Vec::new();
        }

        let all_items = self.list_all().await;
        let mut authorized = Vec::new();

        for item in all_items {
            let access_info =
                MediaAccessInfo::new(item.id.clone(), item.is_child_safe, item.age_rating);

            if authorizer
                .authorize_media_access(ctx, &access_info)
                .await
                .is_ok()
            {
                authorized.push(item);
            }
        }

        authorized
    }

    /// Liste uniquement les films accessibles selon le contexte de requête et le contrôle parental.
    pub async fn list_movies_for_context(
        &self,
        ctx: &RequestContext,
        authorizer: &dyn Authorizer,
    ) -> Vec<MediaItem> {
        if authorizer.authorize_request(ctx).await.is_err() {
            return Vec::new();
        }

        let all_movies = self.db.list_all_movies().await.unwrap_or_default();
        let mut authorized = Vec::new();

        for item in all_movies {
            let access_info =
                MediaAccessInfo::new(item.id.clone(), item.is_child_safe, item.age_rating);

            if authorizer
                .authorize_media_access(ctx, &access_info)
                .await
                .is_ok()
            {
                authorized.push(item);
            }
        }

        authorized
    }

    /// Liste l'ensemble des séries autorisées pour le profil appelant.
    pub async fn list_shows_for_context(
        &self,
        ctx: &RequestContext,
        authorizer: &dyn Authorizer,
    ) -> Vec<ShowRecord> {
        if authorizer.authorize_request(ctx).await.is_err() {
            return Vec::new();
        }

        let all_shows = self.db.list_all_shows().await.unwrap_or_default();
        let mut authorized = Vec::new();

        for show in all_shows {
            if ctx.is_child() {
                if !show.is_child_safe {
                    continue;
                }
                if let (Some(max_age), Some(show_age)) = (ctx.age_rating_max, show.age_rating) {
                    if show_age > max_age {
                        continue;
                    }
                }
            }
            authorized.push(show);
        }

        authorized
    }

    /// Récupère le détail d'une série avec ses saisons, sous réserve d'autorisation.
    pub async fn get_show_details_for_context(
        &self,
        show_id: &ShowId,
        ctx: &RequestContext,
        authorizer: &dyn Authorizer,
    ) -> Result<Option<ShowDetails>, NorylumError> {
        authorizer.authorize_request(ctx).await?;

        let show = match self.db.get_show_by_id(show_id).await? {
            Some(s) => s,
            None => return Ok(None),
        };

        // Contrôle parental fail-closed sur la série
        if ctx.is_child() {
            if !show.is_child_safe {
                return Err(NorylumError::Forbidden(format!(
                    "Accès refusé à la série '{}' : contenu adulte interdit",
                    show.title
                )));
            }
            if let (Some(max_age), Some(show_age)) = (ctx.age_rating_max, show.age_rating) {
                if show_age > max_age {
                    return Err(NorylumError::Forbidden(format!(
                        "Accès refusé à la série '{}' : limite d'âge dépassée",
                        show.title
                    )));
                }
            }
        }

        let seasons = self.db.list_seasons_for_show(show_id).await?;
        Ok(Some(ShowDetails { show, seasons }))
    }

    /// Récupère le détail d'une saison avec ses épisodes, sous réserve d'autorisation.
    pub async fn get_season_details_for_context(
        &self,
        show_id: &ShowId,
        season_number: u32,
        ctx: &RequestContext,
        authorizer: &dyn Authorizer,
    ) -> Result<Option<SeasonDetails>, NorylumError> {
        authorizer.authorize_request(ctx).await?;

        // 1. Vérification de la série et des droits parentaux
        let show = match self.db.get_show_by_id(show_id).await? {
            Some(s) => s,
            None => return Ok(None),
        };

        if ctx.is_child() {
            if !show.is_child_safe {
                return Err(NorylumError::Forbidden(format!(
                    "Accès refusé à la saison de la série '{}' : contenu adulte interdit",
                    show.title
                )));
            }
            if let (Some(max_age), Some(show_age)) = (ctx.age_rating_max, show.age_rating) {
                if show_age > max_age {
                    return Err(NorylumError::Forbidden(format!(
                        "Accès refusé à la saison de la série '{}' : limite d'âge dépassée",
                        show.title
                    )));
                }
            }
        }

        // 2. Recherche de la saison
        let season = match self.db.get_season_by_number(show_id, season_number).await? {
            Some(s) => s,
            None => return Ok(None),
        };

        // 3. Récupération des épisodes avec filtrage parental individuel si besoin
        let raw_episodes = self.db.list_episodes_for_season(&season.id).await?;
        let mut episodes = Vec::new();

        for ep in raw_episodes {
            if ctx.is_child() {
                if !ep.is_child_safe {
                    continue;
                }
                if let (Some(max_age), Some(ep_age)) = (ctx.age_rating_max, ep.age_rating) {
                    if ep_age > max_age {
                        continue;
                    }
                }
            }
            episodes.push(ep);
        }

        Ok(Some(SeasonDetails { season, episodes }))
    }

    /// Nombre de médias indexés.
    pub async fn count(&self) -> usize {
        self.db.count_media().await.unwrap_or(0)
    }
}

impl Default for CatalogService {
    fn default() -> Self {
        Self::in_memory()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use norylum_authorization::DefaultAuthorizer;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_catalog_movies_and_shows_filtering() {
        let catalog = CatalogService::in_memory();
        let authorizer = DefaultAuthorizer::new();

        let movie_kid = MediaItem {
            id: MediaId::new("med_peppa"),
            title: "Peppa Pig Le Film".into(),
            file_path: PathBuf::from("/media/Peppa Pig (2020).mp4"),
            relative_path: "Peppa Pig (2020).mp4".into(),
            file_size: 1000,
            duration_seconds: 3600.0,
            mime_type: "video/mp4".into(),
            video_codec: Some("h264".into()),
            audio_codec: Some("aac".into()),
            width: Some(1920),
            height: Some(1080),
            is_child_safe: true,
            age_rating: Some(3),
        };

        let movie_adult = MediaItem {
            id: MediaId::new("med_matrix"),
            title: "The Matrix".into(),
            file_path: PathBuf::from("/media/The Matrix (1999).mp4"),
            relative_path: "The Matrix (1999).mp4".into(),
            file_size: 5000,
            duration_seconds: 7200.0,
            mime_type: "video/mp4".into(),
            video_codec: Some("hevc".into()),
            audio_codec: Some("ac3".into()),
            width: Some(3840),
            height: Some(2160),
            is_child_safe: false,
            age_rating: Some(16),
        };

        let ep_kid = MediaItem {
            id: MediaId::new("med_bece_01"),
            title: "Bécé Bégé - S01E01 - Tel est pris".into(),
            file_path: PathBuf::from(
                "/media/Bécé Bégé/Saison 01/Bécé Bégé - S01E01 - Tel est pris.mkv",
            ),
            relative_path: "Bécé Bégé/Saison 01/Bécé Bégé - S01E01 - Tel est pris.mkv".into(),
            file_size: 2000,
            duration_seconds: 1200.0,
            mime_type: "video/x-matroska".into(),
            video_codec: Some("h264".into()),
            audio_codec: Some("aac".into()),
            width: Some(1920),
            height: Some(1080),
            is_child_safe: true,
            age_rating: Some(0),
        };

        let ep_adult = MediaItem {
            id: MediaId::new("med_got_01"),
            title: "Game of Thrones - S01E01 - Winter is Coming".into(),
            file_path: PathBuf::from(
                "/media/Game of Thrones/Season 1/Game of Thrones - S01E01 - Winter is Coming.mkv",
            ),
            relative_path:
                "Game of Thrones/Season 1/Game of Thrones - S01E01 - Winter is Coming.mkv".into(),
            file_size: 3000,
            duration_seconds: 3600.0,
            mime_type: "video/x-matroska".into(),
            video_codec: Some("hevc".into()),
            audio_codec: Some("ac3".into()),
            width: Some(1920),
            height: Some(1080),
            is_child_safe: false,
            age_rating: Some(18),
        };

        catalog
            .replace_all(vec![
                movie_kid.clone(),
                movie_adult.clone(),
                ep_kid.clone(),
                ep_adult.clone(),
            ])
            .await;

        let child_ctx = RequestContext::authenticated("acc_1", "prof_kid", "dev_tv", true, Some(6));
        let adult_ctx = RequestContext::authenticated("acc_1", "prof_dad", "dev_pc", false, None);

        // 1. Filtrage films
        let child_movies = catalog
            .list_movies_for_context(&child_ctx, &authorizer)
            .await;
        assert_eq!(child_movies.len(), 1);
        assert_eq!(child_movies[0].id, movie_kid.id);

        let adult_movies = catalog
            .list_movies_for_context(&adult_ctx, &authorizer)
            .await;
        assert_eq!(adult_movies.len(), 2);

        // 2. Filtrage séries
        let child_shows = catalog
            .list_shows_for_context(&child_ctx, &authorizer)
            .await;
        assert_eq!(child_shows.len(), 1);
        assert_eq!(child_shows[0].title, "Bécé Bégé");

        let adult_shows = catalog
            .list_shows_for_context(&adult_ctx, &authorizer)
            .await;
        assert_eq!(adult_shows.len(), 2);

        // 3. Détails d'une série
        let bece_show_id = child_shows[0].id.clone();
        let show_details = catalog
            .get_show_details_for_context(&bece_show_id, &child_ctx, &authorizer)
            .await
            .unwrap();
        assert!(show_details.is_some());
        let sd = show_details.unwrap();
        assert_eq!(sd.seasons.len(), 1);
        assert_eq!(sd.seasons[0].season_number, 1);

        // 4. Détails d'une saison
        let season_details = catalog
            .get_season_details_for_context(&bece_show_id, 1, &child_ctx, &authorizer)
            .await
            .unwrap();
        assert!(season_details.is_some());
        let s_det = season_details.unwrap();
        assert_eq!(s_det.episodes.len(), 1);
        assert_eq!(s_det.episodes[0].media_id, ep_kid.id);

        // 5. Tentative d'accès enfant à la série adulte -> 403 Forbidden
        let got_show = adult_shows
            .iter()
            .find(|s| s.title == "Game of Thrones")
            .unwrap();
        let got_forbidden = catalog
            .get_show_details_for_context(&got_show.id, &child_ctx, &authorizer)
            .await;
        assert!(matches!(got_forbidden, Err(NorylumError::Forbidden(_))));
    }
}
