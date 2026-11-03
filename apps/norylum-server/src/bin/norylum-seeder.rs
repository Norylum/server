//! Explicit opt-in BitTorrent seeding.
//! Uses an isolated snapshot, never the library original.

use librqbit::{
    limits::LimitsConfig, CreateTorrentOptions, ListenerOptions, Session, SessionOptions,
};
use norylum_foundation::MediaId;
use norylum_persistence::Database;
use norylum_torrent::{prepare_snapshot, SeedLock};
use std::{env, fs, net::SocketAddr, num::NonZeroU32, path::PathBuf};

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let id = env::var("NORYLUM_TORRENT_MEDIA_ID")
        .map_err(|_| "Seeding desactive : aucun MediaId configure")?;

    if id.trim().is_empty() {
        return Err("Seeding desactive : aucun MediaId configure".into());
    }

    let limit_kib: u32 = env::var("NORYLUM_TORRENT_UPLOAD_KIBPS")
        .unwrap_or_else(|_| "64".into())
        .parse()?;

    if !(8..=512).contains(&limit_kib) {
        return Err("Debit autorise : 8 a 512 Kio/s".into());
    }

    let rate = NonZeroU32::new(limit_kib * 1024).ok_or("Limite de debit incorrecte")?;

    let root =
        fs::canonicalize(env::var("NORYLUM_MEDIA_DIR").unwrap_or_else(|_| "./media".into()))?;

    // Independent of the server lock: reads the library,
    // writes only into the isolated snapshot directory.
    let _seed_lock = SeedLock::acquire(&root)?;

    let db = Database::open(root.join("norylum.sqlite3"))?;
    let media_id = MediaId::new(id);

    let item = db
        .get_media_by_id(&media_id)
        .await?
        .ok_or("Media non indexe dans Norylum")?;

    if db.embedded_tags_for_media(&media_id).await?.is_none() {
        return Err("Association TMDB requise avant le partage".into());
    }

    let snapshot = prepare_snapshot(&root, &media_id, &item.file_path)?;

    let mut options = SessionOptions::default();
    options.ipv4_only = true;
    options.disable_local_service_discovery = true;
    options.peer_limit = Some(10);
    options.persistence = None;

    options.ratelimits = LimitsConfig {
        upload_bps: Some(rate),
        download_bps: None,
    };

    options.listen = Some(ListenerOptions {
        listen_addr: SocketAddr::from(([0, 0, 0, 0], 6881)),
        enable_upnp_port_forwarding: false,
        ipv4_only: true,
        ..Default::default()
    });

    // No trackers are injected by Norylum.
    // Public DHT is available once seeding is enabled.
    let folder: PathBuf = snapshot
        .parent()
        .ok_or("Dossier snapshot introuvable")?
        .to_path_buf();

    let session = Session::new_with_opts(folder, options).await?;

    let (created, _handle) = session
        .create_and_serve_torrent(&snapshot, CreateTorrentOptions::default())
        .await?;

    let metainfo_path = root
        .join(".norylum-torrent-snapshots")
        .join(format!("{}.torrent", media_id.as_str()));

    fs::write(&metainfo_path, created.as_bytes()?)?;

    println!("Norylum Torrent V1");
    println!("Media : {}", media_id);
    println!("Snapshot : {}", snapshot.display());
    println!("Limite : {} Kio/s", limit_kib);
    println!("Torrent : {}", metainfo_path.display());
    println!("Magnet : {}", created.as_magnet());
    println!("Seeding actif. Arret par SIGTERM ou Ctrl+C.");

    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }

    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;

    session.stop().await;
    println!("Seeding arrete.");

    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Norylum Seeder : {error}");
        std::process::exit(1);
    }
}
