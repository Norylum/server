//! Crate `catalog` pour Norylum Server.
//! Maintient l'inventaire persistant des médias indexés dans SQLite
//! et fournit des fonctions de recherche filtrées par les politiques d'autorisation.

use norylum_authorization::{Authorizer, MediaAccessInfo};
use norylum_foundation::{MediaId, RequestContext};
use norylum_library::MediaItem;
use norylum_persistence::Database;

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
        // Validation globale préliminaire du contexte
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
    async fn test_catalog_sqlite_persistence() {
        let catalog = CatalogService::in_memory();
        let authorizer = DefaultAuthorizer::new();

        let kid_item = MediaItem {
            id: MediaId::new("med_peppa"),
            title: "Peppa Pig".into(),
            file_path: PathBuf::from("/media/peppa.mp4"),
            relative_path: "peppa.mp4".into(),
            file_size: 1000,
            duration_seconds: 300.0,
            mime_type: "video/mp4".into(),
            video_codec: Some("h264".into()),
            audio_codec: Some("aac".into()),
            width: Some(1920),
            height: Some(1080),
            is_child_safe: true,
            age_rating: Some(3),
        };

        let adult_item = MediaItem {
            id: MediaId::new("med_matrix"),
            title: "The Matrix".into(),
            file_path: PathBuf::from("/media/matrix.mp4"),
            relative_path: "matrix.mp4".into(),
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

        catalog
            .replace_all(vec![kid_item.clone(), adult_item.clone()])
            .await;

        let child_ctx = RequestContext::authenticated("acc_1", "prof_kid", "dev_tv", true, Some(6));
        let adult_ctx = RequestContext::authenticated("acc_1", "prof_dad", "dev_pc", false, None);

        // Enfant ne voit que Peppa Pig
        let child_list = catalog.list_for_context(&child_ctx, &authorizer).await;
        assert_eq!(child_list.len(), 1);
        assert_eq!(child_list[0].id, kid_item.id);

        // Adulte voit les deux
        let adult_list = catalog.list_for_context(&adult_ctx, &authorizer).await;
        assert_eq!(adult_list.len(), 2);

        // Vérification de la persistance unitaire par ID
        let fetched = catalog.get_by_id(&kid_item.id).await;
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().title, "Peppa Pig");
    }
}
