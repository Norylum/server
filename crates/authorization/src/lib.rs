//! Crate `authorization` pour Norylum Server.
//! Définit le trait d'autorisation central et sa mise en œuvre "fail-closed" par défaut.

use async_trait::async_trait;
use norylum_foundation::{MediaId, NorylumError, RequestContext};
use serde::{Deserialize, Serialize};

/// Informations minimales nécessaires pour vérifier les permissions d'accès à un média.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaAccessInfo {
    pub media_id: MediaId,
    pub is_child_safe: bool,
    pub age_rating: Option<u32>,
}

impl MediaAccessInfo {
    pub fn new(media_id: impl Into<MediaId>, is_child_safe: bool, age_rating: Option<u32>) -> Self {
        Self {
            media_id: media_id.into(),
            is_child_safe,
            age_rating,
        }
    }
}

/// Trait d'autorisation central du serveur Norylum.
#[async_trait]
pub trait Authorizer: Send + Sync {
    /// Vérifie si la requête est authentifiée et valide (politique fail-closed).
    async fn authorize_request(&self, ctx: &RequestContext) -> Result<(), NorylumError>;

    /// Vérifie si le contexte de requête est autorisé à accéder au média spécifié.
    async fn authorize_media_access(
        &self,
        ctx: &RequestContext,
        media: &MediaAccessInfo,
    ) -> Result<(), NorylumError>;
}

/// Implémentation par défaut de l'Authorizer suivant une stricte politique fail-closed.
#[derive(Debug, Default, Clone)]
pub struct DefaultAuthorizer;

impl DefaultAuthorizer {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Authorizer for DefaultAuthorizer {
    async fn authorize_request(&self, ctx: &RequestContext) -> Result<(), NorylumError> {
        // Règle 1 : Toute requête non explicitement authentifiée est refusée immédiatement (Fail-Closed)
        if !ctx.is_authenticated() {
            tracing::warn!(
                account_id = %ctx.account_id(),
                "Rejet d'autorisation : Contexte non authentifié"
            );
            return Err(NorylumError::Unauthorized(
                "Authentification requise pour accéder aux ressources Norylum".into(),
            ));
        }

        // Règle 2 : Identifiants obligatoires non vides
        if ctx.account_id().as_str().trim().is_empty()
            || ctx.profile_id().as_str().trim().is_empty()
        {
            return Err(NorylumError::Unauthorized(
                "Identifiants de compte ou profil invalides".into(),
            ));
        }

        Ok(())
    }

    async fn authorize_media_access(
        &self,
        ctx: &RequestContext,
        media: &MediaAccessInfo,
    ) -> Result<(), NorylumError> {
        // Validation préliminaire de la requête
        self.authorize_request(ctx).await?;

        // Règle de sécurité enfant :
        if ctx.is_child() {
            if !media.is_child_safe {
                tracing::warn!(
                    profile_id = %ctx.profile_id(),
                    media_id = %media.media_id,
                    "Rejet de sécurité enfant : le média n'est pas balisé comme child-safe"
                );
                return Err(NorylumError::Forbidden(format!(
                    "Accès refusé au média '{}' : profil enfant non autorisé",
                    media.media_id
                )));
            }

            if let (Some(max_age), Some(media_age)) = (ctx.age_rating_max, media.age_rating) {
                if media_age > max_age {
                    tracing::warn!(
                        profile_id = %ctx.profile_id(),
                        media_id = %media.media_id,
                        media_age,
                        max_age,
                        "Rejet de sécurité enfant : limite d'âge dépassée"
                    );
                    return Err(NorylumError::Forbidden(format!(
                        "Accès refusé au média '{}' : classification d'âge {} supérieure au maximum autorisé ({})",
                        media.media_id, media_age, max_age
                    )));
                }
            }
        }

        tracing::debug!(
            profile_id = %ctx.profile_id(),
            media_id = %media.media_id,
            "Accès au média autorisé avec succès"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_unauthenticated_request_fails_closed() {
        let authorizer = DefaultAuthorizer::new();
        let ctx = RequestContext::unauthenticated();

        let result = authorizer.authorize_request(&ctx).await;
        assert!(matches!(result, Err(NorylumError::Unauthorized(_))));
    }

    #[tokio::test]
    async fn test_child_profile_access_rules() {
        let authorizer = DefaultAuthorizer::new();

        let adult_ctx = RequestContext::authenticated("acc_1", "prof_adult", "dev_1", false, None);
        let child_ctx =
            RequestContext::authenticated("acc_1", "prof_child", "dev_2", true, Some(10));

        let mature_media = MediaAccessInfo::new("med_action", false, Some(16));
        let kids_media = MediaAccessInfo::new("med_cartoon", true, Some(6));
        let teen_media = MediaAccessInfo::new("med_teen", true, Some(12));

        // Adulte a accès à tout
        assert!(authorizer
            .authorize_media_access(&adult_ctx, &mature_media)
            .await
            .is_ok());
        assert!(authorizer
            .authorize_media_access(&adult_ctx, &kids_media)
            .await
            .is_ok());

        // Enfant n'a pas accès au média non child-safe
        assert!(matches!(
            authorizer
                .authorize_media_access(&child_ctx, &mature_media)
                .await,
            Err(NorylumError::Forbidden(_))
        ));

        // Enfant a accès au média child-safe <= 10 ans
        assert!(authorizer
            .authorize_media_access(&child_ctx, &kids_media)
            .await
            .is_ok());

        // Enfant n'a pas accès au média child-safe avec classification > 10 ans
        assert!(matches!(
            authorizer
                .authorize_media_access(&child_ctx, &teen_media)
                .await,
            Err(NorylumError::Forbidden(_))
        ));
    }
}
