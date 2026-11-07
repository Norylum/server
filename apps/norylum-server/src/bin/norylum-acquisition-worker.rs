//! Acquisition worker V4 : offline smoke test, auto-search via providers, et téléchargement
//! HTTPS ou BitTorrent sous VPN Gluetun.

use librqbit::{
    limits::LimitsConfig, AddTorrent, AddTorrentOptions, ListenerOptions, Session, SessionOptions,
};
use norylum_acquisition_providers::{Provider, ProviderQuery, ProwlarrProvider, SourceType};
use norylum_acquisition_transport::download_https;
use norylum_foundation::{AccountId, ProfileId};
use norylum_persistence::{AcquisitionLease, Database};
use reqwest::Url;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const GLUETUN_STATUS_API: &str = "http://127.0.0.1:8000/v1/vpn/status";
const RENEWAL_INTERVAL_SECS: u64 = 60;
const TORRENT_POLL_SECS: u64 = 15;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(String::as_str) {
        Some("--offline-smoke") => run_smoke().await,
        Some("--run") => run_acquisition().await,
        _ => {
            eprintln!(
                "Usage: norylum-acquisition-worker --offline-smoke | --run\n\
                 --offline-smoke : validation SQLite hors reseau\n\
                 --run           : traitement d'une acquisition sous VPN (necessite NORYLUM_MEDIA_DIR)"
            );
            std::process::exit(2);
        }
    }
}

async fn run_smoke() -> Result<(), Box<dyn std::error::Error>> {
    let db = Database::in_memory()?;
    let account = AccountId::new("acc_offline_smoke");
    let profile = ProfileId::new("prof_offline_smoke");

    let request = db
        .request_acquisition(
            &account, &profile, "movie", 987650103, None, None, "1080p", "fr",
        )
        .await?;

    db.approve_acquisition_request(&account, &request.id)
        .await?;

    db.register_acquisition_source(
        &account,
        &request.id,
        "https://cdn.example.org/offline-test.mp4",
        &"c".repeat(64),
        &["cdn.example.org".to_owned()],
    )
    .await?;

    let lease = db
        .claim_next_approved_acquisition()
        .await?
        .ok_or("Aucune reservation obtenue")?;

    assert_eq!(lease.request_id, request.id);
    assert_eq!(lease.attempt, 1);

    db.renew_acquisition_claim(&lease).await?;
    db.finish_acquisition_claim(&lease, "unavailable").await?;

    let finished = db
        .get_acquisition_request(&account, &request.id)
        .await?
        .ok_or("Demande de test introuvable")?;

    assert_eq!(finished.status, "unavailable");

    // Smoke test du chemin auto-search (sans providers réels).
    let request2 = db
        .request_acquisition(
            &account, &profile, "movie", 987650104, None, None, "1080p", "fr",
        )
        .await?;
    db.approve_acquisition_request(&account, &request2.id).await?;

    let unsourced_lease = db.claim_next_unsourced_acquisition().await?
        .ok_or("Aucune reservation non-sourcee")?;
    assert_eq!(unsourced_lease.request_id, request2.id);

    db.register_torrent_source_via_lease(
        &unsourced_lease,
        "magnet:?xt=urn:btih:aabbccdd00112233aabbccdd00112233aabbccdd",
    ).await?;

    db.finish_acquisition_claim(&unsourced_lease, "failed").await?;

    println!("OK : worker hors reseau, reservation et cloture SQLite validees");
    println!("Aucun fichier media ni telechargement n'a ete cree.");

    Ok(())
}

async fn run_acquisition() -> Result<(), Box<dyn std::error::Error>> {
    let media_dir = PathBuf::from(
        std::env::var("NORYLUM_MEDIA_DIR").unwrap_or_else(|_| "./media".to_owned()),
    );

    // 1. VPN must be active before any claim or network operation.
    check_vpn().await?;

    // 2. Open SQLite on the shared media volume.
    let db_path = media_dir.join("norylum.sqlite3");
    let db = Database::open(&db_path)?;

    // 3. Quarantine directory: same volume as staging for atomic rename.
    let quarantine = media_dir.join(".norylum-acq-quarantine");
    prepare_quarantine(&quarantine)?;

    // 4. Try the auto-search path first if Prowlarr is configured.
    let prowlarr = ProwlarrProvider::from_env();
    if let Some(provider) = prowlarr {
        if try_auto_search(&db, &provider, &media_dir, &quarantine).await? {
            return Ok(());
        }
    }

    // 5. Fall back to the manual-source path (HTTPS or pre-registered torrent).
    try_sourced(&db, &media_dir, &quarantine).await
}

/// Auto-search path: claim an unsourced request, query providers, register + download.
/// Returns true if work was found and processed (success or failure).
async fn try_auto_search(
    db: &Database,
    provider: &impl Provider,
    media_dir: &Path,
    quarantine: &Path,
) -> Result<bool, Box<dyn std::error::Error>> {
    let Some(lease) = db.claim_next_unsourced_acquisition().await? else {
        return Ok(false);
    };

    println!("Acquisition non-sourcee reservee : {}", lease.request_id);

    // Load request details to build the provider query.
    let req = match find_request_for_lease(db, &lease).await? {
        Some(r) => r,
        None => {
            eprintln!("ERREUR : demande {} introuvable", lease.request_id);
            let _ = db.finish_acquisition_claim(&lease, "failed").await;
            return Ok(true);
        }
    };

    let query = ProviderQuery {
        tmdb_id: req.tmdb_id,
        kind: req.kind.clone(),
        quality_profile: req.quality_profile.clone(),
        language: req.language.clone(),
    };

    let results = provider.search(query).await;

    // Pick best result: most seeders first, then first found.
    let best = results
        .into_iter()
        .max_by_key(|r| r.seeders.unwrap_or(0));

    let Some(result) = best else {
        eprintln!("Aucun résultat fournisseur pour {}", lease.request_id);
        let _ = db.finish_acquisition_claim(&lease, "unavailable").await;
        return Ok(true);
    };

    println!("Source trouvée : {} ({})", result.title, result.url);

    // Register the found source under the active lease.
    if let Err(e) = db.register_torrent_source_via_lease(&lease, &result.url).await {
        eprintln!("ERREUR enregistrement source : {e}");
        let _ = db.finish_acquisition_claim(&lease, "failed").await;
        return Ok(true);
    }

    // Download based on source type.
    match result.source_type {
        SourceType::Torrent => {
            download_and_stage_torrent(db, &lease, &result.url, media_dir, quarantine).await?;
        }
        SourceType::Https => {
            // HTTPS from provider: sha256 not available, mark unavailable.
            eprintln!("HTTPS depuis fournisseur sans SHA-256 : non supporté, marquer unavailable");
            let _ = db.finish_acquisition_claim(&lease, "unavailable").await;
        }
    }

    Ok(true)
}

/// Manual-source path: claim a request with an already-registered source and download it.
async fn try_sourced(
    db: &Database,
    media_dir: &Path,
    quarantine: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(lease) = db.claim_next_approved_acquisition().await? else {
        println!("OK : aucune acquisition en attente");
        return Ok(());
    };

    println!("Acquisition sourcée reservee : {}", lease.request_id);

    let source = match db.source_for_acquisition_claim(&lease).await? {
        Some(s) => s,
        None => {
            eprintln!("ERREUR : source introuvable pour {}", lease.request_id);
            let _ = db.finish_acquisition_claim(&lease, "failed").await;
            return Ok(());
        }
    };

    if source.source_type == "torrent" {
        download_and_stage_torrent(db, &lease, &source.source_url, media_dir, quarantine).await
    } else {
        download_and_stage_https(db, &lease, &source, media_dir, quarantine).await
    }
}

/// HTTPS download path (unchanged from V3.3).
async fn download_and_stage_https(
    db: &Database,
    lease: &AcquisitionLease,
    source: &norylum_persistence::AuthorizedAcquisitionSource,
    media_dir: &Path,
    quarantine: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let host = Url::parse(&source.source_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .ok_or("URL de source sans domaine")?;
    let allowed_hosts = vec![host];
    let staging_name = derive_staging_name(&source.source_url);
    let staging_dir = media_dir.join(".norylum-staging");

    let db_renew = db.clone();
    let lease_renew = lease.clone();
    let renewal = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(RENEWAL_INTERVAL_SECS));
        interval.tick().await;
        loop {
            interval.tick().await;
            if db_renew.renew_acquisition_claim(&lease_renew).await.is_err() {
                eprintln!("ATTENTION : renouvellement du bail echoue, abandon");
                break;
            }
        }
    });

    let download_result = download_https(source, &allowed_hosts, quarantine).await;
    renewal.abort();
    let _ = renewal.await;

    let artifact = match download_result {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ERREUR telechargement HTTPS : {e}");
            let _ = db.finish_acquisition_claim(lease, "failed").await;
            return Ok(());
        }
    };

    println!("Téléchargement HTTPS OK : {} octets, SHA-256 vérifié", artifact.bytes());

    if let Err(e) = stage_artifact(artifact.path(), &staging_dir, &staging_name) {
        eprintln!("ERREUR staging : {e}");
        let _ = db.finish_acquisition_claim(lease, "failed").await;
        return Ok(());
    }

    println!("Staged : {staging_name}");
    let request_id = lease.request_id.clone();
    db.finish_acquisition_claim(lease, "available").await?;
    fire_webhook(request_id);
    println!("OK : acquisition {} disponible pour indexation automatique", lease.request_id);
    Ok(())
}

/// Vérifie qu'une IP résolue ne pointe pas vers une ressource interne.
fn is_blocked_ip(ip: std::net::IpAddr) -> bool {
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || matches!(ip, std::net::IpAddr::V4(v4) if
            v4.is_private() || v4.is_link_local() || v4.is_broadcast()
            // 100.64/10 (CGNAT), 169.254/16 (link-local) déjà couverts ci-dessus
        )
        || matches!(ip, std::net::IpAddr::V6(v6) if
            v6.is_unicast_link_local()
            // fd00::/8 (ULA private) — préfixe 0xFD
            || (v6.octets()[0] == 0xfd)
        )
}

/// Pour les URLs HTTPS .torrent : télécharge le fichier nous-mêmes avec DNS pré-résolu et
/// IP pinnée (protège contre le rebinding DNS), puis retourne les bytes.
/// Pour les magnet URIs : retourne None — librqbit les gère sans requête HTTP directe.
async fn resolve_torrent_add(
    url: &str,
) -> Result<AddTorrent<'static>, Box<dyn std::error::Error>> {
    if url.starts_with("magnet:") {
        return Ok(AddTorrent::from_url(url.to_owned()));
    }

    let parsed = Url::parse(url).map_err(|_| "URL torrent mal formée")?;

    if parsed.scheme() != "https" {
        return Err("URL torrent non-HTTPS refusée".into());
    }
    if parsed.port().is_some() {
        return Err("URL torrent avec port non standard refusée".into());
    }

    let host = parsed.host_str().ok_or("URL torrent sans hôte")?.to_owned();

    // Défense en profondeur : rejeter les noms internes sans résolution.
    let host_lc = host.to_ascii_lowercase();
    if host_lc == "localhost"
        || host_lc.ends_with(".local")
        || host_lc.ends_with(".localhost")
        || host_lc.ends_with(".internal")
    {
        return Err(format!("URL torrent vers hôte interne refusée : {host}").into());
    }

    // Résolution DNS : valider TOUTES les IPs retournées, puis pinner la première.
    let addrs: Vec<_> = tokio::net::lookup_host((&*host, 443u16))
        .await
        .map_err(|e| format!("Résolution DNS échouée pour {host} : {e}"))?
        .collect();

    if addrs.is_empty() {
        return Err(format!("Aucune adresse résolue pour {host}").into());
    }

    for addr in &addrs {
        if is_blocked_ip(addr.ip()) {
            return Err(format!(
                "URL torrent résolue vers IP protégée refusée : {} → {}",
                host,
                addr.ip()
            )
            .into());
        }
    }

    // Pinner la première IP résolue pour éviter le rebinding DNS entre validation et fetch.
    let pinned = addrs[0];

    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .resolve(&host, pinned)
        // Rejeter les redirections : un fichier .torrent n'a pas de raison légitime
        // de rediriger, et une redirection pourrait pointer vers une IP interne en
        // contournant le DNS pinning ci-dessus.
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let bytes = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Téléchargement .torrent échoué : {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("Lecture bytes .torrent échouée : {e}"))?;

    Ok(AddTorrent::from_bytes(bytes))
}

/// BitTorrent download path via librqbit.
async fn download_and_stage_torrent(
    db: &Database,
    lease: &AcquisitionLease,
    magnet_or_url: &str,
    media_dir: &Path,
    quarantine: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    // For HTTPS .torrent URLs: download ourselves with DNS pre-resolution + IP pinning,
    // then pass bytes to librqbit. Magnet URIs are passed through directly (no HTTP request).
    let add_torrent = match resolve_torrent_add(magnet_or_url).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ERREUR résolution source torrent : {e}");
            let _ = db.finish_acquisition_claim(lease, "failed").await;
            return Ok(());
        }
    };

    let staging_dir = media_dir.join(".norylum-staging");

    let session = Arc::new(
        Session::new_with_opts(
            quarantine.to_path_buf(),
            SessionOptions {
                ipv4_only: true,
                disable_local_service_discovery: true,
                peer_limit: Some(50),
                ratelimits: LimitsConfig {
                    upload_bps: None,
                    download_bps: None,
                },
                listen: Some(ListenerOptions {
                    listen_addr: SocketAddr::from(([0, 0, 0, 0], 6882)),
                    enable_upnp_port_forwarding: false,
                    ipv4_only: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await?,
    );

    let response = session
        .add_torrent(
            add_torrent,
            Some(AddTorrentOptions {
                output_folder: Some(quarantine.to_string_lossy().to_string()),
                ..Default::default()
            }),
        )
        .await?;

    let handle = response
        .into_handle()
        .ok_or("Impossible d'obtenir un handle de torrent")?;

    println!("Torrent démarré : {magnet_or_url}");

    // Poll until finished, renewing lease every 60 s.
    let mut lease_timer = tokio::time::interval(Duration::from_secs(RENEWAL_INTERVAL_SECS));
    lease_timer.tick().await; // discard immediate tick

    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(TORRENT_POLL_SECS)) => {
                let stats = handle.stats();

                if let Some(err) = stats.error.as_deref() {
                    eprintln!("ERREUR torrent : {err}");
                    let _ = db.finish_acquisition_claim(lease, "failed").await;
                    session.stop().await;
                    return Ok(());
                }

                if stats.finished {
                    println!(
                        "Téléchargement torrent OK : {} octets",
                        stats.progress_bytes
                    );
                    break;
                }

                println!("Torrent en cours : {stats}");
            }
            _ = lease_timer.tick() => {
                if db.renew_acquisition_claim(lease).await.is_err() {
                    eprintln!("ATTENTION : renouvellement du bail echoue, abandon torrent");
                    session.stop().await;
                    return Ok(());
                }
            }
        }
    }

    session.stop().await;

    // Find the largest downloaded file in quarantine as the media file.
    let staged_file = find_largest_file(quarantine)?;
    let staging_name = staged_file
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Nom de fichier torrent invalide")?
        .to_owned();

    // Validate extension.
    let lower = staging_name.to_ascii_lowercase();
    if !lower.ends_with(".mkv") && !lower.ends_with(".mp4") && !lower.ends_with(".m4v") {
        eprintln!("ERREUR : extension non supportée : {staging_name}");
        let _ = db.finish_acquisition_claim(lease, "failed").await;
        return Ok(());
    }

    if let Err(e) = stage_artifact(&staged_file, &staging_dir, &staging_name) {
        eprintln!("ERREUR staging torrent : {e}");
        let _ = db.finish_acquisition_claim(lease, "failed").await;
        return Ok(());
    }

    println!("Staged : {staging_name}");
    let request_id = lease.request_id.clone();
    db.finish_acquisition_claim(lease, "available").await?;
    fire_webhook(request_id);
    println!(
        "OK : acquisition {} disponible pour indexation automatique",
        lease.request_id
    );
    Ok(())
}

/// Charge les détails d'une demande pour construire la requête fournisseur.
async fn find_request_for_lease(
    db: &Database,
    lease: &AcquisitionLease,
) -> Result<Option<norylum_persistence::AcquisitionRequest>, Box<dyn std::error::Error>> {
    // On récupère toutes les demandes du système pour trouver celle qui correspond au bail.
    // Le bail ne stocke pas le account_id, on doit l'inférer via SQL direct.
    let id = lease.request_id.clone();
    let token = lease.lease_token.clone();

    let req = db
        .find_acquisition_request_by_lease(&id, &token)
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    Ok(req)
}

async fn check_vpn() -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()?;

    let body: serde_json::Value = client
        .get(GLUETUN_STATUS_API)
        .send()
        .await
        .map_err(|e| format!("API Gluetun inaccessible sur localhost:8000 : {e}"))?
        .json()
        .await
        .map_err(|_| "Reponse Gluetun non JSON")?;

    if body.get("status").and_then(|v| v.as_str()) != Some("running") {
        return Err(format!("VPN non actif : statut = {:?}", body.get("status")).into());
    }

    println!("VPN actif confirme via Gluetun (statut : running)");
    Ok(())
}

fn prepare_quarantine(quarantine: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if !quarantine.exists() {
        std::fs::create_dir_all(quarantine)?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(quarantine, std::fs::Permissions::from_mode(0o700))?;
    }

    let meta = std::fs::symlink_metadata(quarantine)?;

    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("Le repertoire quarantaine est un lien symbolique ou absent".into());
    }

    Ok(())
}

fn stage_artifact(
    source: &Path,
    staging_dir: &Path,
    name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if !staging_dir.is_dir() {
        std::fs::create_dir_all(staging_dir)?;
    }

    let target = staging_dir.join(name);

    if target.exists() {
        return Err(format!(
            "Fichier staging deja present : {}",
            target.display()
        )
        .into());
    }

    std::fs::rename(source, &target)?;

    let ready = staging_dir.join(format!("{name}.ready"));
    std::fs::File::create(&ready)
        .map_err(|e| format!("Impossible de creer le marqueur .ready : {e}"))?;

    Ok(())
}

fn derive_staging_name(url: &str) -> String {
    if let Ok(parsed) = Url::parse(url) {
        if let Some(segment) = parsed.path_segments().and_then(|mut s| s.next_back()) {
            let clean: String = segment
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                .collect();

            let valid_ext = clean.ends_with(".mkv")
                || clean.ends_with(".mp4")
                || clean.ends_with(".m4v");

            if !clean.is_empty() && clean.len() <= 200 && valid_ext {
                return clean;
            }
        }
    }

    format!("norylum-acq-{}.mp4", uuid::Uuid::new_v4().as_simple())
}

/// Fire-and-forget webhook on acquisition success.
/// Reads `NORYLUM_WEBHOOK_URL` from the environment; silently does nothing if unset.
fn fire_webhook(request_id: String) {
    let url = match std::env::var("NORYLUM_WEBHOOK_URL") {
        Ok(u) if !u.is_empty() => u,
        _ => return,
    };
    tokio::spawn(async move {
        let body = serde_json::json!({
            "event": "acquisition_completed",
            "request_id": request_id,
        });
        let result = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap()
            .post(&url)
            .json(&body)
            .send()
            .await;
        if let Err(e) = result {
            eprintln!("WARN : webhook échoué : {e}");
        }
    });
}

fn find_largest_file(dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut best: Option<(u64, PathBuf)> = None;

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            let size = meta.len();
            if best.as_ref().is_none_or(|(s, _)| size > *s) {
                best = Some((size, entry.path()));
            }
        }
    }

    best.map(|(_, p)| p)
        .ok_or_else(|| "Aucun fichier dans le répertoire quarantaine".into())
}
