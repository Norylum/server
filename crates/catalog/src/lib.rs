//! Crate `catalog` pour Norylum Server.
//! Maintient l'inventaire en mémoire des médias indexés et fournit
//! des fonctions de recherche filtrées par les politiques d'autorisation.

use norylum_authorization::{Authorizer, MediaAccessInfo};
use norylum_foundation::{MediaId, RequestContext};
use norylum_library::MediaItem;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Service central de catalogue média pour Norylum.
#[derive(Debug, Default, Clone)]
pub struct CatalogService {
    items: Arc<RwLock<HashMap<MediaId, MediaItem>>>,
}

impl CatalogService {
    pub fn new() -> Self {
        Self {
            items: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Ajoute ou met à jour un média dans le catalogue.
    pub async fn add_item(&self, item: MediaItem) {
        let mut map = self.items.write().await;
        map.insert(item.id.clone(), item);
    }

    /// Remplace ou fusionne un lot d'éléments indexés.
    pub async fn replace_all(&self, new_items: Vec<MediaItem>) {
        let mut map = self.items.write().await;
        map.clear();
        for item in new_items {
            map.insert(item.id.clone(), item);
        }
    }

    /// Récupère un média par son identifiant unique.
    pub async fn get_by_id(&self, id: &MediaId) -> Option<MediaItem> {
        let map = self.items.read().await;
        map.get(id).cloned()
    }

    /// Liste l'ensemble des médias du catalogue (sans contrôle de contexte).
    pub async fn list_all(&self) -> Vec<MediaItem> {
        let map = self.items.read().await;
        map.values().cloned().collect()
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

        let map = self.items.read().await;
        let mut authorized = Vec::new();

        for item in map.values() {
            let access_info =
                MediaAccessInfo::new(item.id.clone(), item.is_child_safe, item.age_rating);

            if authorizer
                .authorize_media_access(ctx, &access_info)
                .await
                .is_ok()
            {
                authorized.push(item.clone());
            }
        }

        authorized
    }

    /// Nombre de médias indexés.
    pub async fn count(&self) -> usize {
        let map = self.items.read().await;
        map.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use norylum_authorization::DefaultAuthorizer;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_catalog_filtering_by_context() {
        let catalog = CatalogService::new();
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

        catalog.add_item(kid_item.clone()).await;
        catalog.add_item(adult_item.clone()).await;

        let child_ctx = RequestContext::authenticated("acc_1", "prof_kid", "dev_tv", true, Some(6));
        let adult_ctx = RequestContext::authenticated("acc_1", "prof_dad", "dev_pc", false, None);
        let unauth_ctx = RequestContext::unauthenticated();

        // Enfant ne voit que Peppa Pig
        let child_list = catalog.list_for_context(&child_ctx, &authorizer).await;
        assert_eq!(child_list.len(), 1);
        assert_eq!(child_list[0].id, kid_item.id);

        // Adulte voit les deux
        let adult_list = catalog.list_for_context(&adult_ctx, &authorizer).await;
        assert_eq!(adult_list.len(), 2);

        // Non authentifié ne voit rien
        let unauth_list = catalog.list_for_context(&unauth_ctx, &authorizer).await;
        assert_eq!(unauth_list.len(), 0);
    }
}
