//! Automatic ingestion, but only for files with an explicit .ready marker.
//! Polling survives missed filesystem notifications and process restarts.
use crate::{ingestion_routes::process_ready, AppState};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_MARKERS_PER_PASS: usize = 128;
const MAX_MARKERS_COLLECTED: usize = 10_000;

fn marker_source(staging: &Path, marker: &Path) -> Option<String> {
    let relative = marker.strip_prefix(staging).ok()?;
    let value = relative.to_str()?;
    let source = value.strip_suffix(".ready")?;
    if source.is_empty()
        || source.len() > 4096
        || source.starts_with('/')
        || source.contains('\\')
        || source.split('/').any(|p| {
            p.is_empty()
                || p == "."
                || p == ".."
                || p.starts_with('.')
                || p.chars().any(char::is_control)
        })
    {
        return None;
    }
    let ext = Path::new(source)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "mkv" | "mp4" | "m4v" | "mov" | "avi" | "webm" | "ts" | "wmv"
    )
    .then(|| source.to_owned())
}

fn collect_markers(staging: &Path) -> io::Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    let mut directories = vec![staging.to_owned()];
    while let Some(dir) = directories.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                if directories.len() < 512 {
                    directories.push(entry.path());
                }
            } else if meta.is_file()
                && entry.path().extension().and_then(|s| s.to_str()) == Some("ready")
            {
                result.push(entry.path());
                if result.len() >= MAX_MARKERS_COLLECTED {
                    return Ok(result);
                }
            }
        }
    }
    result.sort();
    Ok(result)
}

#[cfg(unix)]
fn same_inode(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let (Ok(a), Ok(b)) = (fs::symlink_metadata(a), fs::symlink_metadata(b)) else {
        return false;
    };
    a.is_file() && b.is_file() && a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
fn same_inode(_a: &Path, _b: &Path) -> bool {
    false
}

async fn process_marker(state: &AppState, staging: &Path, marker: &Path) {
    let Some(relative) = marker_source(staging, marker) else {
        tracing::warn!(marker = ?marker, "Marqueur ready invalide ignore");
        return;
    };
    let source = staging.join(&relative);
    let destination = state.library_root.join(&relative);
    let db = state.playback.db();
    let recovery_guard = state.scan_lock.lock().await;

    // Recover from a crash after indexing but before deleting the hardlinked
    // staging entry. Never remove a destination unless it is demonstrably the
    // same inode as our staging source.
    // Never inspect or unlink a destination through an untrusted parent symlink.
    if destination.exists()
        && !destination
            .parent()
            .and_then(|p| fs::canonicalize(p).ok())
            .is_some_and(|p| p.starts_with(&state.library_root))
    {
        tracing::warn!(path = %relative, "Destination externe ou parent non fiable");
        return;
    }
    if same_inode(&source, &destination) {
        match db.ingestion_media_for_path(&relative).await {
            Ok(Some(media)) => {
                if let Err(e) = fs::remove_file(&source) {
                    tracing::warn!(error = %e, "Reprise : nettoyage staging impossible");
                    return;
                }
                if let Err(e) = db.finish_ingestion(&relative, Ok(&media.id)).await {
                    tracing::warn!(error = %e, "Reprise : journal non actualise");
                }
                let _ = fs::remove_file(marker);
                tracing::info!(path = %relative, "Ingestion recuperee apres interruption");
                return;
            }
            Ok(None) => {
                // The file was linked but never indexed: rollback and retry.
                if let Err(e) = fs::remove_file(&destination) {
                    tracing::warn!(error = %e, "Reprise : rollback impossible");
                    return;
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Reprise SQLite impossible");
                return;
            }
        }
    } else if !source.exists() && destination.is_file() {
        if let Ok(Some(_)) = db.ingestion_media_for_path(&relative).await {
            let _ = fs::remove_file(marker);
            return;
        }
    }

    drop(recovery_guard);
    match process_ready(state, &relative, &relative).await {
        Ok(result) => {
            if result.staging_removed {
                if let Err(e) = fs::remove_file(marker) {
                    tracing::warn!(marker = ?marker, error = %e, "Marqueur a nettoyer");
                }
            }
            tracing::info!(path = %relative, "Media publie automatiquement");
        }
        Err(e) => {
            tracing::debug!(path = %relative, error = %e, "Ingestion differee ou en erreur");
        }
    }
}

pub async fn run_ingestion_worker(state: AppState) {
    let seconds = std::env::var("NORYLUM_INGEST_POLL_SECONDS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(15)
        .clamp(5, 3600);
    let mut interval = tokio::time::interval(Duration::from_secs(seconds));
    let staging = state.library_root.join(".norylum-staging");
    let mut cursor = 0usize;
    loop {
        interval.tick().await;
        let Ok(staging_meta) = fs::symlink_metadata(&staging) else {
            continue;
        };
        if staging_meta.file_type().is_symlink() || !staging_meta.is_dir() {
            continue;
        }
        let markers = match collect_markers(&staging) {
            Ok(markers) => markers,
            Err(e) => {
                tracing::warn!(error = %e, "Exploration staging impossible");
                continue;
            }
        };
        if markers.is_empty() {
            continue;
        }
        // Rotate the queue so failing files cannot starve later arrivals.
        let start = cursor % markers.len();
        for index in 0..markers.len().min(MAX_MARKERS_PER_PASS) {
            process_marker(&state, &staging, &markers[(start + index) % markers.len()]).await;
        }
        cursor = (start + MAX_MARKERS_PER_PASS) % markers.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_only_ready_video_markers() {
        let base = Path::new("/tmp/staging");
        assert_eq!(
            marker_source(base, &base.join("Series/S01E02.mkv.ready")),
            Some("Series/S01E02.mkv".into())
        );
        assert!(marker_source(base, &base.join("../bad.mkv.ready")).is_none());
        assert!(marker_source(base, &base.join("bad.txt.ready")).is_none());
        assert!(marker_source(base, &base.join(".hidden.mkv.ready")).is_none());
    }
}
