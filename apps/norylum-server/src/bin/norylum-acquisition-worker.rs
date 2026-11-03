//! Acquisition worker bootstrap: offline-only until VPN egress is proven.
//! No HTTP client, filesystem staging, torrent session or production DB.

use norylum_foundation::{AccountId, ProfileId};
use norylum_persistence::Database;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    if arguments.len() != 1 || arguments[0] != "--offline-smoke" {
        eprintln!("Acquisition worker bloque : seul --offline-smoke est autorise");
        std::process::exit(2);
    }

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
