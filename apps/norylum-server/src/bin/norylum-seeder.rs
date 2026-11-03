//! Torrent V2: persistent opt-in multi-torrent coordinator.
//! A single librqbit session shares one global upload limit.

use librqbit::{
    limits::LimitsConfig, CreateTorrentOptions, ListenerOptions, ManagedTorrent, Session,
    SessionOptions,
};
use norylum_foundation::MediaId;
use norylum_persistence::{Database, TorrentShare};
use norylum_torrent::{prepare_snapshot, SeedLock};
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    net::SocketAddr,
    num::NonZeroU32,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

struct Tracked {
    handle: Arc<ManagedTorrent>,
    active: bool,
    previous_uploaded: u64,
    last_sample: Instant,
}

async fn start_session(
    root: &Path,
    limit: u32,
) -> Result<Arc<Session>, Box<dyn std::error::Error>> {
    let options = SessionOptions {
        ipv4_only: true,
        disable_local_service_discovery: true,
        peer_limit: Some(10),
        persistence: None,
        ratelimits: LimitsConfig {
            upload_bps: Some(NonZeroU32::new(limit * 1024).ok_or("Limite d'envoi invalide")?),
            download_bps: None,
        },
        listen: Some(ListenerOptions {
            listen_addr: SocketAddr::from(([0, 0, 0, 0], 6881)),
            enable_upnp_port_forwarding: false,
            ipv4_only: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    let folder = root.join(".norylum-torrent-snapshots");

    Ok(Session::new_with_opts(folder, options).await?)
}

async fn add_seed(
    db: &Database,
    root: &Path,
    session: &Arc<Session>,
    share: &TorrentShare,
) -> Result<Tracked, Box<dyn std::error::Error>> {
    let id = MediaId::new(share.media_id.clone());

    let item = db.get_media_by_id(&id).await?.ok_or("Media non indexe")?;

    if db.embedded_tags_for_media(&id).await?.is_none() {
        return Err("Association TMDB absente".into());
    }

    let snapshot = prepare_snapshot(root, &id, &item.file_path)?;

    let (created, handle) = session
        .create_and_serve_torrent(&snapshot, CreateTorrentOptions::default())
        .await?;

    let metainfo = root
        .join(".norylum-torrent-snapshots")
        .join(format!("{}.torrent", id.as_str()));

    fs::write(metainfo, created.as_bytes()?)?;

    let magnet = created.as_magnet().to_string();

    db.set_torrent_runtime_state(&id, true, "seeding", Some(magnet.as_str()), None)
        .await?;

    println!("Torrent actif: {} - {}", id, created.as_magnet());

    Ok(Tracked {
        handle,
        active: true,
        previous_uploaded: 0,
        last_sample: Instant::now(),
    })
}

async fn reconcile(
    db: &Database,
    root: &Path,
    session: &mut Option<Arc<Session>>,
    current_limit: &mut Option<u32>,
    tracked: &mut HashMap<String, Tracked>,
    retry_after: &mut HashMap<String, Instant>,
) -> Result<(), Box<dyn std::error::Error>> {
    let shares = db.torrent_shares().await?;
    let wanted: HashSet<String> = shares
        .iter()
        .filter(|s| s.enabled)
        .map(|s| s.media_id.clone())
        .collect();

    // Stop unwanted torrents before any new torrent is added.
    for (key, item) in tracked.iter_mut() {
        if !wanted.contains(key) && item.active {
            if let Some(running) = session.as_ref() {
                running.pause(&item.handle).await?;
            }
            item.active = false;

            let id = MediaId::new(key.clone());
            db.set_torrent_runtime_state(&id, false, "disabled", None, None)
                .await?;
        }
    }

    // Disabled rows without an in-memory handle have no peers.
    for share in &shares {
        if !share.enabled && share.status != "disabled" {
            db.set_torrent_runtime_state(
                &MediaId::new(share.media_id.clone()),
                false,
                "disabled",
                None,
                None,
            )
            .await?;
        }
    }

    if wanted.is_empty() {
        if let Some(running) = session.take() {
            running.stop().await;
        }
        tracked.clear();
        retry_after.clear();
        *current_limit = None;
        return Ok(());
    }

    let limit = db.torrent_settings().await?.upload_kibps;

    // Changing the global cap requires one controlled session restart.
    if current_limit.is_some_and(|old| old != limit) {
        if let Some(running) = session.take() {
            running.stop().await;
        }
        tracked.clear();
        retry_after.clear();
        *current_limit = None;
    }

    if session.is_none() {
        *session = Some(start_session(root, limit).await?);
        *current_limit = Some(limit);
        println!("Limite globale : {limit} Kio/s");
    }

    let running = session.as_ref().ok_or("Session torrent indisponible")?;

    // The limit applies to the entire session, not to each torrent.
    for share in shares.iter().filter(|s| s.enabled) {
        let key = share.media_id.clone();
        let id = MediaId::new(key.clone());

        if let Some(item) = tracked.get_mut(&key) {
            if !item.active {
                running.unpause(&item.handle).await?;
                item.active = true;
                item.previous_uploaded = item.handle.stats().uploaded_bytes;
                item.last_sample = Instant::now();
            }
        } else {
            if retry_after
                .get(&key)
                .is_some_and(|at| at.elapsed() < Duration::from_secs(60))
            {
                continue;
            }

            match add_seed(db, root, running, share).await {
                Ok(item) => {
                    tracked.insert(key.clone(), item);
                    retry_after.remove(&key);
                }
                Err(error) => {
                    let message = error.to_string();
                    eprintln!("Torrent {} : {}", key, message);

                    db.set_torrent_runtime_state(&id, true, "error", None, Some(&message))
                        .await?;

                    retry_after.insert(key, Instant::now());
                    continue;
                }
            }
        }

        if let Some(item) = tracked.get_mut(&key) {
            let stats = item.handle.stats();
            let now = Instant::now();

            let elapsed_ms = u64::try_from(now.duration_since(item.last_sample).as_millis())
                .unwrap_or(u64::MAX)
                .max(1);

            let delta = stats.uploaded_bytes.saturating_sub(item.previous_uploaded);

            let rate = delta.saturating_mul(1000) / elapsed_ms;

            item.previous_uploaded = stats.uploaded_bytes;
            item.last_sample = now;

            if let Some(error) = stats.error.as_deref() {
                db.set_torrent_runtime_state(&id, true, "error", None, Some(error))
                    .await?;
            } else {
                // Peer count is intentionally unknown until a verified
                // peer-stats adapter is added. Never invent a count.
                db.record_torrent_metrics(&id, delta, rate, None).await?;
            }
        }
    }

    Ok(())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let root =
        fs::canonicalize(env::var("NORYLUM_MEDIA_DIR").unwrap_or_else(|_| "./media".into()))?;

    let _lock = SeedLock::acquire(&root)?;
    let db = Database::open(root.join("norylum.sqlite3"))?;

    let mut session: Option<Arc<Session>> = None;
    let mut current_limit = None;
    let mut tracked = HashMap::new();
    let mut retry_after = HashMap::new();

    println!("Norylum Torrent V2 pret.");
    println!("Aucun partage sans autorisation SQLite.");

    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    loop {
        #[cfg(unix)]
        let shutdown = async {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
        };

        #[cfg(not(unix))]
        let shutdown = tokio::signal::ctrl_c();

        tokio::select! {
            _ = shutdown => break,
            _ = tokio::time::sleep(Duration::from_secs(5)) => {
                if let Err(error) = reconcile(
                    &db,
                    &root,
                    &mut session,
                    &mut current_limit,
                    &mut tracked,
                    &mut retry_after,
                ).await {
                    eprintln!("Torrent V2 : {error}");
                }
            }
        }
    }

    if let Some(running) = session.take() {
        running.stop().await;
    }

    println!("Seeder arrete.");
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Erreur Torrent V2 : {error}");
        std::process::exit(1);
    }
}
