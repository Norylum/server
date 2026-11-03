//! Explicit opt-in E2E test of real seeder and cleanup executables.
//! Everything lives under a temporary directory.
//! The seeder runs loopback-only, without DHT, trackers or LSD.

use norylum_foundation::MediaId;
use norylum_library::MediaItem;
use norylum_metadata::{TmdbItem, TmdbKind};
use norylum_persistence::{Database, TorrentShare};
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, SystemTime},
};

struct Seeder(Child);

impl Seeder {
    fn stop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for Seeder {
    fn drop(&mut self) {
        self.stop();
    }
}

fn spawn_seeder(root: &Path, log_path: &Path) -> Seeder {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .unwrap();

    let error_log = log.try_clone().unwrap();

    let child = Command::new(env!("CARGO_BIN_EXE_norylum-seeder"))
        .env("NORYLUM_MEDIA_DIR", root)
        .env("NORYLUM_TORRENT_E2E_LOCAL_ONLY", "1")
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(error_log))
        .spawn()
        .unwrap();

    Seeder(child)
}

fn cleanup(root: &Path, id: &MediaId, apply: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_norylum-torrent-cleanup"));

    command
        .env("NORYLUM_MEDIA_DIR", root)
        .arg("--media-id")
        .arg(id.as_str());

    if apply {
        command.arg("--apply");
    }

    command.output().unwrap()
}

async fn wait_for_status(
    db: &Database,
    id: &MediaId,
    wanted: &str,
    log_path: &Path,
) -> TorrentShare {
    for _ in 0..60 {
        let shares = db.torrent_shares().await.unwrap();

        if let Some(share) = shares
            .into_iter()
            .find(|share| share.media_id == id.as_str())
        {
            if share.status == wanted {
                return share;
            }

            if share.status == "error" {
                panic!(
                    "Seeder en erreur : {:?}\n{}",
                    share.last_error,
                    fs::read_to_string(log_path).unwrap_or_default()
                );
            }
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    panic!(
        "Statut {wanted} non atteint.\n{}",
        fs::read_to_string(log_path).unwrap_or_default()
    );
}

fn synthetic_item(id: MediaId, path: PathBuf, size: usize) -> MediaItem {
    MediaItem {
        id,
        title: "Norylum Synthetic E2E".into(),
        file_path: path,
        relative_path: "Norylum Synthetic E2E (2026).mkv".into(),
        file_size: size as u64,
        duration_seconds: 1.0,
        mime_type: "video/x-matroska".into(),
        video_codec: None,
        audio_codec: None,
        width: None,
        height: None,
        is_child_safe: true,
        age_rating: Some(0),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lance explicitement un seeder local isole"]
async fn synthetic_torrent_lifecycle() {
    // No production path, media, DB, accounts or credentials.
    let temp = std::env::temp_dir().join(format!("norylum-torrent-e2e-{}", uuid::Uuid::new_v4()));

    fs::create_dir(&temp).unwrap();
    let root = fs::canonicalize(&temp).unwrap();

    // Required by the seeder's test-only safety guard.
    fs::write(root.join(".norylum-torrent-e2e"), b"test-only").unwrap();

    let id = MediaId::new("med_0123456789abcdef");
    let filename = "Norylum Synthetic E2E (2026).mkv";
    let original = root.join(filename);
    let log_path = root.join("seeder-e2e.log");

    let bytes = b"SYNTHETIC NORYLUM E2E TEST DATA".repeat(4096);
    fs::write(&original, &bytes).unwrap();

    // prepare_snapshot requires the source to be at least 30s old.
    fs::File::open(&original)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(120))
        .unwrap();

    let db_path = root.join("norylum.sqlite3");
    let db = Database::open(&db_path).unwrap();

    let item = synthetic_item(id.clone(), original.clone(), bytes.len());

    db.sync_media_items(&[item]).await.unwrap();

    // Local synthetic association: no TMDB API request.
    db.set_movie_metadata(
        &id,
        &TmdbItem {
            tmdb_id: 603,
            tmdb_ref: "movie/603".into(),
            kind: TmdbKind::Movie,
            title: "Synthetic E2E".into(),
            overview: String::new(),
            poster_path: None,
            poster_url: None,
            release_date: None,
        },
    )
    .await
    .unwrap();

    let snapshot = root
        .join(".norylum-torrent-snapshots")
        .join(id.as_str())
        .join(filename);

    let metainfo = root
        .join(".norylum-torrent-snapshots")
        .join(format!("{}.torrent", id.as_str()));

    // 1. Boot the real seeder with no authorized share.
    let mut seeder = spawn_seeder(&root, &log_path);

    tokio::time::sleep(Duration::from_secs(7)).await;

    assert!(seeder.0.try_wait().unwrap().is_none());
    assert!(db.torrent_shares().await.unwrap().is_empty());
    assert!(!snapshot.exists());
    assert!(!metainfo.exists());

    // 2. Explicitly opt in, on the isolated SQLite database.
    db.set_torrent_enabled(&id, true).await.unwrap();

    let active = wait_for_status(&db, &id, "seeding", &log_path).await;

    assert!(active.enabled);
    assert!(active.magnet_uri.is_some());
    assert_eq!(fs::read(&snapshot).unwrap(), bytes);
    assert!(metainfo.is_file());

    // 3. The cleanup binary must refuse an active seeder.
    let denied = cleanup(&root, &id, true);
    assert!(
        !denied.status.success(),
        "Nettoyage accepte pendant le seeding"
    );
    assert!(snapshot.exists());
    assert_eq!(fs::read(&original).unwrap(), bytes);

    // 4. Disable and wait until the real coordinator acknowledges it.
    db.set_torrent_enabled(&id, false).await.unwrap();

    let disabled = wait_for_status(&db, &id, "disabled", &log_path).await;

    assert!(!disabled.enabled);
    assert_eq!(disabled.upload_bps, 0);

    // Ensure the process releases its filesystem lock.
    seeder.stop();

    // 5. A dry run must not delete anything.
    let dry_run = cleanup(&root, &id, false);
    assert!(
        dry_run.status.success(),
        "Dry run refuse : {}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
    assert!(snapshot.is_file());

    // 6. Authorized cleanup removes only the snapshot.
    let applied = cleanup(&root, &id, true);
    assert!(
        applied.status.success(),
        "Nettoyage refuse : {}",
        String::from_utf8_lossy(&applied.stderr)
    );

    assert!(!snapshot.exists());
    assert!(!metainfo.exists());
    assert_eq!(fs::read(&original).unwrap(), bytes);

    // 7. SQLite state persists across a new connection.
    let reopened = Database::open(&db_path).unwrap();
    let shares = reopened.torrent_shares().await.unwrap();

    assert_eq!(shares.len(), 1);
    assert!(!shares[0].enabled);
    assert_eq!(shares[0].status, "disabled");

    drop(reopened);

    // 8. Restart: disabled media must never regenerate a torrent.
    let mut restarted = spawn_seeder(&root, &log_path);

    tokio::time::sleep(Duration::from_secs(7)).await;

    assert!(restarted.0.try_wait().unwrap().is_none());
    assert!(!snapshot.exists());
    assert!(!metainfo.exists());

    restarted.stop();

    assert_eq!(fs::read(&original).unwrap(), bytes);

    drop(db);
    fs::remove_dir_all(&root).unwrap();

    println!("OK : cycle E2E synthetique et isole complet.");
}
