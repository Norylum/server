//! Acquisition worker: offline smoke test and VPN-gated HTTPS download.

use norylum_acquisition_transport::download_https;
use norylum_foundation::{AccountId, ProfileId};
use norylum_persistence::Database;
use reqwest::Url;
use std::path::{Path, PathBuf};
use std::time::Duration;

const GLUETUN_STATUS_API: &str = "http://127.0.0.1:8000/v1/vpn/status";
const RENEWAL_INTERVAL_SECS: u64 = 60;

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

    // 4. Claim the oldest approved request that has an authorized source.
    let Some(lease) = db.claim_next_approved_acquisition().await? else {
        println!("OK : aucune acquisition en attente");
        return Ok(());
    };

    println!("Acquisition reservee : {}", lease.request_id);

    // 5. Retrieve the source record (requires a live lease).
    let source = match db.source_for_acquisition_claim(&lease).await? {
        Some(s) => s,
        None => {
            eprintln!("ERREUR : source introuvable pour {}", lease.request_id);
            let _ = db.finish_acquisition_claim(&lease, "failed").await;
            return Ok(());
        }
    };

    // 6. Allowed host derived from the already-validated source URL.
    let host = Url::parse(&source.source_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .ok_or("URL de source sans domaine")?;
    let allowed_hosts = vec![host];

    // 7. Determine the filename for the staging directory.
    let staging_name = derive_staging_name(&source.source_url);
    let staging_dir = media_dir.join(".norylum-staging");

    // 8. Renew the lease every 60 s while downloading (lease TTL is 90 s).
    let db_renew = db.clone();
    let lease_renew = lease.clone();
    let renewal = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(RENEWAL_INTERVAL_SECS));
        interval.tick().await; // discard the immediate first tick
        loop {
            interval.tick().await;
            if db_renew
                .renew_acquisition_claim(&lease_renew)
                .await
                .is_err()
            {
                eprintln!("ATTENTION : renouvellement du bail echoue, abandon");
                break;
            }
        }
    });

    // 9. Download with SSRF protection and streaming SHA-256 verification.
    let download_result = download_https(&source, &allowed_hosts, &quarantine).await;

    renewal.abort();
    let _ = renewal.await;

    let artifact = match download_result {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ERREUR telechargement : {e}");
            let _ = db.finish_acquisition_claim(&lease, "failed").await;
            return Ok(());
        }
    };

    println!(
        "Telechargement OK : {} octets, SHA-256 verifie",
        artifact.bytes()
    );

    // 10. Rename artifact into staging atomically and create the .ready marker.
    if let Err(e) = stage_artifact(artifact.path(), &staging_dir, &staging_name) {
        eprintln!("ERREUR staging : {e}");
        let _ = db.finish_acquisition_claim(&lease, "failed").await;
        return Ok(());
    }

    println!("Staged : {staging_name}");

    // 11. Mark the acquisition as available for the ingestion pipeline.
    db.finish_acquisition_claim(&lease, "available").await?;

    println!(
        "OK : acquisition {} disponible pour indexation automatique",
        lease.request_id
    );

    Ok(())
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

    // Atomic on the same filesystem (quarantine and staging are both under media/).
    std::fs::rename(source, &target)?;

    // The VerifiedArtifact Drop will silently fail to remove the renamed path.

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

    format!(
        "norylum-acq-{}.mp4",
        uuid::Uuid::new_v4().as_simple()
    )
}
