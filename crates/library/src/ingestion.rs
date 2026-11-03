//! Explicit staging readiness notification and no-overwrite publication.
//! Only a trusted local process should put files in .norylum-staging.

use norylum_foundation::NorylumError;
use serde_json::Value;
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use tokio::{process::Command, time::sleep};

const STAGING: &str = ".norylum-staging";
const STABILITY_WAIT: Duration = Duration::from_secs(2);
const MIN_AGE: Duration = Duration::from_secs(30);

fn validation(message: impl Into<String>) -> NorylumError {
    NorylumError::Validation(message.into())
}

fn conflict(message: impl Into<String>) -> NorylumError {
    NorylumError::Conflict(message.into())
}

fn validated_relative(value: &str) -> Result<PathBuf, NorylumError> {
    if value.is_empty()
        || value.len() > 4096
        || value.starts_with('/')
        || value.contains('\\')
        || value.split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || part.starts_with('.')
                || part.chars().any(char::is_control)
        })
    {
        return Err(validation("Chemin relatif invalide ou cache"));
    }
    Ok(PathBuf::from(value))
}

fn supported_ext(path: &Path) -> Result<String, NorylumError> {
    let ext = path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "mkv" | "mp4" | "m4v" | "mov" | "avi" | "webm" | "ts" | "wmv"
    ) {
        return Err(validation("Extension video non prise en charge"));
    }
    Ok(ext)
}

fn ensure_no_links(base: &Path, relative: &Path) -> Result<PathBuf, NorylumError> {
    let mut path = base.to_path_buf();
    for component in relative.components() {
        path.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&path).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                NorylumError::NotFound("Fichier staging introuvable".into())
            } else {
                NorylumError::Io(e)
            }
        })?;
        if metadata.file_type().is_symlink() {
            return Err(validation("Lien symbolique interdit dans le staging"));
        }
    }
    Ok(path)
}

fn ensure_dest_parent(root: &Path, relative: &Path) -> Result<PathBuf, NorylumError> {
    let mut parent = root.to_path_buf();
    if let Some(relative_parent) = relative.parent() {
        for component in relative_parent.components() {
            parent.push(component.as_os_str());
            match fs::symlink_metadata(&parent) {
                Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                    return Err(validation("Parent de destination non fiable"));
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir(&parent)?;
                }
                Err(e) => return Err(NorylumError::Io(e)),
            }
        }
    }
    if !fs::canonicalize(&parent)?.starts_with(root) {
        return Err(validation("Destination hors bibliotheque"));
    }
    Ok(root.join(relative))
}

fn stamp(path: &Path) -> Result<(u64, SystemTime), NorylumError> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_file() {
        return Err(validation("Source non reguliere ou lien symbolique"));
    }
    Ok((meta.len(), meta.modified()?))
}

async fn strict_probe(path: &Path) -> Result<(), NorylumError> {
    let mut command = Command::new("ffprobe");
    command
        .kill_on_drop(true)
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type",
            "-of",
            "json",
        ])
        .arg(path);
    let output = tokio::time::timeout(Duration::from_secs(45), command.output())
        .await
        .map_err(|_| validation("ffprobe a depasse 45 secondes"))?
        .map_err(|e| NorylumError::Internal(format!("ffprobe indisponible : {e}")))?;
    if !output.status.success() {
        return Err(validation(
            "ffprobe refuse le media : fichier incomplet ou corrompu",
        ));
    }
    let data: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| validation(format!("Reponse ffprobe invalide : {e}")))?;
    let has_video = data["streams"]
        .as_array()
        .is_some_and(|streams| streams.iter().any(|s| s["codec_type"] == "video"));
    if !has_video {
        return Err(validation("Aucune piste video detectee"));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct IngestionService {
    root: PathBuf,
}

pub struct PreparedIngestion {
    root: PathBuf,
    source: PathBuf,
    relative_destination: String,
    destination: PathBuf,
    initial_stamp: (u64, SystemTime),
}

pub struct PublishedIngestion {
    source: PathBuf,
    destination: PathBuf,
    relative_destination: String,
    committed: bool,
}

impl IngestionService {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Explicit ready signal; filesystem stability and strict ffprobe are checked.
    pub async fn notify_file_ready(
        &self,
        source_relative: &str,
        destination_relative: &str,
    ) -> Result<PreparedIngestion, NorylumError> {
        let source_relative = validated_relative(source_relative)?;
        let destination_relative_path = validated_relative(destination_relative)?;
        if supported_ext(&source_relative)? != supported_ext(&destination_relative_path)? {
            return Err(validation(
                "Les extensions source et destination doivent correspondre",
            ));
        }

        let root = fs::canonicalize(&self.root)?;
        let staging = root.join(STAGING);
        match fs::symlink_metadata(&staging) {
            Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
                return Err(validation("Repertoire staging non fiable"));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&staging)?;
            }
            Err(e) => return Err(NorylumError::Io(e)),
        }

        let source = ensure_no_links(&staging, &source_relative)?;
        if !fs::canonicalize(&source)?.starts_with(&staging) {
            return Err(validation("Source hors staging"));
        }
        let before = stamp(&source)?;
        if before.0 == 0 {
            return Err(validation("Fichier staging vide"));
        }
        if before.1.elapsed().unwrap_or_default() < MIN_AGE {
            return Err(conflict("Fichier modifie depuis moins de 30 secondes"));
        }

        sleep(STABILITY_WAIT).await;
        if stamp(&source)? != before {
            return Err(conflict(
                "Le fichier change pendant le controle de stabilite",
            ));
        }
        strict_probe(&source).await?;
        if stamp(&source)? != before {
            return Err(conflict("Le fichier change pendant ffprobe"));
        }

        let destination = root.join(&destination_relative_path);
        if destination.exists() || fs::symlink_metadata(&destination).is_ok() {
            return Err(conflict("Destination deja occupee"));
        }

        Ok(PreparedIngestion {
            root,
            source,
            relative_destination: destination_relative.to_string(),
            destination,
            initial_stamp: before,
        })
    }
}

impl PreparedIngestion {
    /// A hard link publishes the complete inode without replacing any existing file.
    /// The staging file is kept until the SQLite transaction succeeds.
    pub fn publish(self) -> Result<PublishedIngestion, NorylumError> {
        if stamp(&self.source)? != self.initial_stamp {
            return Err(conflict("Source modifiee avant publication"));
        }
        let rel = validated_relative(&self.relative_destination)?;
        let destination = ensure_dest_parent(&self.root, &rel)?;
        if destination != self.destination {
            return Err(validation("Destination modifiee"));
        }
        fs::hard_link(&self.source, &destination).map_err(|e| {
            if e.kind() == io::ErrorKind::AlreadyExists {
                conflict("Destination deja occupee")
            } else {
                NorylumError::Io(e)
            }
        })?;
        let published = PublishedIngestion {
            source: self.source,
            destination,
            relative_destination: self.relative_destination,
            committed: false,
        };
        if stamp(&published.source)? != self.initial_stamp {
            return Err(conflict("Source modifiee pendant la publication"));
        }
        Ok(published)
    }
}

impl PublishedIngestion {
    pub fn relative_destination(&self) -> &str {
        &self.relative_destination
    }

    /// Keep the published file only after successful SQLite indexing.
    /// Returns whether staging was successfully cleaned up.
    pub fn commit(mut self) -> bool {
        self.committed = true;
        match fs::remove_file(&self.source) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(path = ?self.source, error = %e, "Staging cleanup pending");
                false
            }
        }
    }
}

impl Drop for PublishedIngestion {
    fn drop(&mut self) {
        if !self.committed {
            if let Err(e) = fs::remove_file(&self.destination) {
                tracing::error!(path = ?self.destination, error = %e, "Ingestion rollback failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_hidden_and_absolute_paths() {
        for bad in [
            "",
            "../escape.mkv",
            "movies/../escape.mkv",
            "/tmp/file.mkv",
            ".norylum-staging/secret.mkv",
            "movies//file.mkv",
            "./file.mkv",
            "movies\\file.mkv",
            "movies/.hidden.mkv",
        ] {
            assert!(validated_relative(bad).is_err(), "accepted {bad:?}");
        }
        assert!(validated_relative("Shows/Season 1/S01E01.mkv").is_ok());
    }

    #[test]
    fn blocks_non_video_and_extension_mismatch() {
        assert!(supported_ext(Path::new("payload.exe")).is_err());
        assert!(supported_ext(Path::new("movie.mkv")).is_ok());
    }
}
