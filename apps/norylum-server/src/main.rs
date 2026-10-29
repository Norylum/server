//! Binaire principal `norylum-server`.
//! Monolithe modulaire haute performance pour l'écosystème Norylum.

use norylum_api::{create_router, AppState};
use norylum_authorization::DefaultAuthorizer;
use norylum_catalog::CatalogService;
use norylum_persistence::Database;
use norylum_playback::PlaybackService;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialisation des logs structurés
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,norylum=debug")),
        )
        .init();

    tracing::info!("Démarrage du serveur Norylum (Monolithe Modulaire - ADR-001)...");

    // Détermination du répertoire média (configuré par NORYLUM_MEDIA_DIR ou fallback ./media)
    let media_dir_str =
        std::env::var("NORYLUM_MEDIA_DIR").unwrap_or_else(|_| "./media".to_string());
    let media_dir = PathBuf::from(&media_dir_str);
    if !media_dir.exists() {
        tracing::info!(path = ?media_dir, "Création du répertoire média par défaut");
        std::fs::create_dir_all(&media_dir)?;
    }
    let canonical_media_dir = std::fs::canonicalize(&media_dir)?;
    tracing::info!(media_dir = ?canonical_media_dir, "Répertoire média configuré");

    // Initialisation de la persistance SQLite local-first (WAL + migrations automatiques)
    let db_path = canonical_media_dir.join("norylum.sqlite3");
    tracing::info!(db_path = ?db_path, "Connexion à la base SQLite Norylum");
    let db = Database::open(&db_path)?;

    // Instanciation des services du domaine adossés à SQLite
    let catalog = Arc::new(CatalogService::new(db.clone()));
    let authorizer = Arc::new(DefaultAuthorizer::new());
    let playback = Arc::new(PlaybackService::new(db));

    // Injection de l'état partagé (remplace les appels HTTP inter-services)
    let app_state = AppState::new(canonical_media_dir, catalog, authorizer, playback);

    // Initialisation du routeur complet
    let app = create_router(app_state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 8120));
    tracing::info!("Serveur Norylum prêt et en écoute sur http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
