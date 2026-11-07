//! Metadonnees enrichies TMDB.
//! Consultation controlee et association manuelle.

use axum::{
    extract::{Path, Query, State},
    Extension, Json,
};
use norylum_authorization::{Authorizer, MediaAccessInfo};
use norylum_catalog::CatalogService;
use norylum_foundation::{MediaId, NorylumError, RequestContext, ShowId};
use norylum_metadata::{parse_relative_path, ParsedName, PosterCache, TmdbClient, TmdbItem, TmdbKind};
use norylum_persistence::{Database, MetadataRecord};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct MetadataState {
    pub catalog: Arc<CatalogService>,
    pub authorizer: Arc<dyn Authorizer>,
    pub db: Database,
    pub tmdb: TmdbClient,
    pub poster_cache: Arc<PosterCache>,
}

#[derive(Deserialize)]
pub(crate) struct SearchQuery {
    kind: TmdbKind,
    query: String,
}

#[derive(Deserialize)]
pub(crate) struct AssociationPayload {
    tmdb_id: u64,
}

async fn principal(state: &MetadataState, ctx: &RequestContext) -> Result<(), NorylumError> {
    state.authorizer.authorize_request(ctx).await?;

    if ctx.is_child()
        || !state
            .db
            .is_principal_profile(ctx.account_id(), ctx.profile_id())
            .await?
    {
        return Err(NorylumError::Forbidden("Profil principal requis".into()));
    }

    Ok(())
}

async fn checked_movie(
    state: &MetadataState,
    ctx: &RequestContext,
    id: &MediaId,
) -> Result<norylum_library::MediaItem, NorylumError> {
    state.authorizer.authorize_request(ctx).await?;

    let media = state
        .catalog
        .get_by_id(id)
        .await
        .ok_or_else(|| NorylumError::NotFound("Media introuvable".into()))?;

    state
        .authorizer
        .authorize_media_access(
            ctx,
            &MediaAccessInfo::new(media.id.clone(), media.is_child_safe, media.age_rating),
        )
        .await?;

    Ok(media)
}

async fn checked_show(
    state: &MetadataState,
    ctx: &RequestContext,
    id: &ShowId,
) -> Result<norylum_persistence::ShowRecord, NorylumError> {
    state.authorizer.authorize_request(ctx).await?;

    let show = state
        .db
        .get_show_by_id(id)
        .await?
        .ok_or_else(|| NorylumError::NotFound("Serie introuvable".into()))?;

    state
        .authorizer
        .authorize_media_access(
            ctx,
            &MediaAccessInfo::new(
                MediaId::new(show.id.as_str()),
                show.is_child_safe,
                show.age_rating,
            ),
        )
        .await?;

    Ok(show)
}

/// GET /v1/metadata/search?kind=movie&query=...
pub(crate) async fn search_handler(
    State(state): State<MetadataState>,
    Extension(ctx): Extension<RequestContext>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<TmdbItem>>, NorylumError> {
    principal(&state, &ctx).await?;

    Ok(Json(state.tmdb.search(query.kind, &query.query).await?))
}

/// GET /v1/metadata/media/:media_id
pub(crate) async fn get_movie_handler(
    State(state): State<MetadataState>,
    Extension(ctx): Extension<RequestContext>,
    Path(media_id): Path<MediaId>,
) -> Result<Json<MetadataRecord>, NorylumError> {
    checked_movie(&state, &ctx, &media_id).await?;

    let mut record = state
        .db
        .get_movie_metadata(&media_id)
        .await?
        .ok_or_else(|| NorylumError::NotFound("Aucune association TMDB pour ce film".into()))?;

    if let Some(url) = record.poster_url.as_deref().and_then(|u| state.poster_cache.local_url_if_cached(u)) {
        record.poster_url = Some(url);
    }

    Ok(Json(record))
}

/// PUT /v1/metadata/media/:media_id
pub(crate) async fn set_movie_handler(
    State(state): State<MetadataState>,
    Extension(ctx): Extension<RequestContext>,
    Path(media_id): Path<MediaId>,
    Json(payload): Json<AssociationPayload>,
) -> Result<Json<MetadataRecord>, NorylumError> {
    principal(&state, &ctx).await?;

    let media = checked_movie(&state, &ctx, &media_id).await?;

    if !matches!(
        parse_relative_path(&media.relative_path),
        ParsedName::Movie { .. }
    ) {
        return Err(NorylumError::Validation(
            "Ce media n'est pas classe comme film".into(),
        ));
    }

    let item = state.tmdb.details(TmdbKind::Movie, payload.tmdb_id).await?;
    let mut record = state.db.set_movie_metadata(&media_id, &item).await?;

    if let Some(path) = item.poster_path.as_deref() {
        if let Some(local_url) = state.poster_cache.store(path).await {
            record.poster_url = Some(local_url);
        }
    }

    Ok(Json(record))
}

/// GET /v1/metadata/shows/:show_id
pub(crate) async fn get_show_handler(
    State(state): State<MetadataState>,
    Extension(ctx): Extension<RequestContext>,
    Path(show_id): Path<ShowId>,
) -> Result<Json<MetadataRecord>, NorylumError> {
    checked_show(&state, &ctx, &show_id).await?;

    let mut record =
        state.db.get_show_metadata(&show_id).await?.ok_or_else(|| {
            NorylumError::NotFound("Aucune association TMDB pour cette serie".into())
        })?;

    if let Some(url) = record.poster_url.as_deref().and_then(|u| state.poster_cache.local_url_if_cached(u)) {
        record.poster_url = Some(url);
    }

    Ok(Json(record))
}

/// PUT /v1/metadata/shows/:show_id
pub(crate) async fn set_show_handler(
    State(state): State<MetadataState>,
    Extension(ctx): Extension<RequestContext>,
    Path(show_id): Path<ShowId>,
    Json(payload): Json<AssociationPayload>,
) -> Result<Json<MetadataRecord>, NorylumError> {
    principal(&state, &ctx).await?;
    checked_show(&state, &ctx, &show_id).await?;

    let item = state.tmdb.details(TmdbKind::Tv, payload.tmdb_id).await?;
    let mut record = state.db.set_show_metadata(&show_id, &item).await?;

    if let Some(path) = item.poster_path.as_deref() {
        if let Some(local_url) = state.poster_cache.store(path).await {
            record.poster_url = Some(local_url);
        }
    }

    Ok(Json(record))
}

/// GET /v1/metadata/media/:media_id/suggestions
/// Suggestions depuis le titre extrait du fichier.
/// Aucune association n'est effectuee ici.
pub(crate) async fn suggest_movie_handler(
    State(state): State<MetadataState>,
    Extension(ctx): Extension<RequestContext>,
    Path(media_id): Path<MediaId>,
) -> Result<Json<Vec<TmdbItem>>, NorylumError> {
    principal(&state, &ctx).await?;

    let media = checked_movie(&state, &ctx, &media_id).await?;

    let title = match parse_relative_path(&media.relative_path) {
        ParsedName::Movie { title, .. } => title,
        _ => return Err(NorylumError::Validation("Titre de film non reconnu".into())),
    };

    Ok(Json(state.tmdb.search(TmdbKind::Movie, &title).await?))
}

/// GET /v1/metadata/shows/:show_id/suggestions
pub(crate) async fn suggest_show_handler(
    State(state): State<MetadataState>,
    Extension(ctx): Extension<RequestContext>,
    Path(show_id): Path<ShowId>,
) -> Result<Json<Vec<TmdbItem>>, NorylumError> {
    principal(&state, &ctx).await?;

    let show = checked_show(&state, &ctx, &show_id).await?;

    Ok(Json(state.tmdb.search(TmdbKind::Tv, &show.title).await?))
}
