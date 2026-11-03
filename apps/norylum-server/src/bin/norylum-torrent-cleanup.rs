//! Offline, guarded deletion of one disabled torrent snapshot.

use norylum_foundation::{MediaId, NorylumError};
use norylum_persistence::Database;
use norylum_torrent::SeedLock;
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use std::{env, fs};

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();

    let (id, apply) = match args.as_slice() {
        [flag, id] if flag == "--media-id" => (id, false),
        [flag, id, confirm] if flag == "--media-id" && confirm == "--apply" => (id, true),
        _ => {
            return Err("Usage: norylum-torrent-cleanup --media-id med_ID [--apply]".into());
        }
    };

    let id = MediaId::new(id.clone());

    let root =
        fs::canonicalize(env::var("NORYLUM_MEDIA_DIR").unwrap_or_else(|_| "./media".into()))?;

    // Fails if the seeder is still running on the same volume.
    let lock = SeedLock::acquire(&root)?;

    let db = Database::open(root.join("norylum.sqlite3"))?;
    let shares = db.torrent_shares().await?;

    if shares.iter().any(|share| share.enabled) {
        return Err("Arreter tous les partages avant le nettoyage".into());
    }

    let share = shares
        .iter()
        .find(|share| share.media_id == id.as_str())
        .ok_or("Partage inconnu dans SQLite")?;

    if share.status != "disabled" {
        return Err("Le partage doit etre confirme disabled".into());
    }

    let candidate = lock.inspect_snapshot(&id)?;
    println!("Snapshot : {}", candidate.display());

    if !apply {
        println!("Mode verification : aucun fichier supprime.");
        println!("Ajouter --apply pour executer le nettoyage.");
        return Ok(());
    }

    // Hold the SQLite write transaction across the filesystem unlink.
    // Concurrent API enable requests cannot be committed during cleanup.
    db.with_connection(|conn| {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        let active: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM torrent_shares WHERE enabled=1",
                [],
                |row| row.get(0),
            )
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        if active != 0 {
            return Err(NorylumError::Conflict("Un torrent a ete reactive".into()));
        }

        let status: Option<String> = tx
            .query_row(
                "SELECT status FROM torrent_shares WHERE media_id=?1",
                params![id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        if status.as_deref() != Some("disabled") {
            return Err(NorylumError::Conflict(
                "Statut torrent modifie pendant le nettoyage".into(),
            ));
        }

        lock.cleanup_snapshot(&id)?;

        tx.commit()
            .map_err(|e| NorylumError::Internal(e.to_string()))?;

        Ok(())
    })
    .await?;

    println!("Snapshot et fichier torrent supprimes.");
    println!("La video originale est conservee.");

    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Nettoyage torrent refuse : {error}");
        std::process::exit(1);
    }
}
