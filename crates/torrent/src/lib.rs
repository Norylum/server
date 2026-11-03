//! Isolated, verified snapshots for opt-in BitTorrent seeding.
//! Never seed a mutable library file directly.

use fs2::FileExt;
use norylum_foundation::{MediaId, NorylumError};
use norylum_library::PathSanitizer;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read},
    path::{Component, Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;

const SNAPSHOTS: &str = ".norylum-torrent-snapshots";
const MIN_AGE: Duration = Duration::from_secs(30);

fn invalid(message: &str) -> NorylumError {
    NorylumError::Validation(message.to_owned())
}

fn conflict(message: &str) -> NorylumError {
    NorylumError::Conflict(message.to_owned())
}

fn check_id(id: &MediaId) -> Result<(), NorylumError> {
    let suffix = id
        .as_str()
        .strip_prefix("med_")
        .ok_or_else(|| invalid("MediaId incorrect"))?;

    if !(16..=32).contains(&suffix.len()) || !suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid("MediaId non canonique"));
    }
    Ok(())
}

fn safe_directory(path: &Path) -> Result<(), NorylumError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(invalid("Repertoire non fiable")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path)?;
            Ok(())
        }
        Err(e) => Err(NorylumError::Io(e)),
    }
}

fn digest(path: &Path) -> Result<[u8; 32], NorylumError> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 128 * 1024];

    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash.finalize().into())
}

pub struct SeedLock {
    _file: File,
}

impl SeedLock {
    /// Exclusivity between cooperating Norylum seeders.
    pub fn acquire(root: &Path) -> Result<Self, NorylumError> {
        let base = fs::canonicalize(root)?;
        safe_directory(&base.join(SNAPSHOTS))?;

        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(base.join(SNAPSHOTS).join(".seeder.lock"))?;

        file.try_lock_exclusive()
            .map_err(|_| conflict("Un seeder Norylum est deja actif"))?;

        Ok(Self { _file: file })
    }
}

pub fn prepare_snapshot(
    root: &Path,
    id: &MediaId,
    requested: &Path,
) -> Result<PathBuf, NorylumError> {
    prepare_with_age(root, id, requested, MIN_AGE)
}

fn prepare_with_age(
    root: &Path,
    id: &MediaId,
    requested: &Path,
    min_age: Duration,
) -> Result<PathBuf, NorylumError> {
    check_id(id)?;

    // macOS peut exposer /var comme alias de /private/var.
    // Conserver la racine fournie pour reconnaitre les deux formes.
    let input_root = root;
    let root = fs::canonicalize(input_root)?;

    if !requested.is_absolute() {
        return Err(invalid("Chemin source non absolu"));
    }

    let relative = requested
        .strip_prefix(&root)
        .or_else(|_| requested.strip_prefix(input_root))
        .map_err(|_| invalid("Source hors bibliotheque"))?;

    if !relative.components().all(|c| match c {
        Component::Normal(s) => s
            .to_str()
            .is_some_and(|v| !v.starts_with('.') && !v.chars().any(char::is_control)),
        _ => false,
    }) {
        return Err(invalid("Chemin source non fiable"));
    }

    let mut cursor = root.clone();
    for part in relative.components() {
        cursor.push(part.as_os_str());
        let meta = fs::symlink_metadata(&cursor)?;
        if meta.file_type().is_symlink() {
            return Err(invalid("Lien symbolique interdit"));
        }
    }

    let source = PathSanitizer::sanitize_path(&root, requested)?;
    let original = fs::metadata(&source)?;

    if !original.is_file() || original.len() == 0 {
        return Err(invalid("Source non reguliere ou vide"));
    }

    if original.modified()?.elapsed().unwrap_or_default() < min_age {
        return Err(conflict("Fichier source trop recent"));
    }

    let ext = source
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if !matches!(ext.as_str(), "mkv" | "mp4" | "m4v") {
        return Err(invalid("Seuls MKV et MP4/M4V sont partageables"));
    }

    let snapshots = root.join(SNAPSHOTS);
    safe_directory(&snapshots)?;

    let folder = snapshots.join(id.as_str());
    safe_directory(&folder)?;

    let name = source
        .file_name()
        .ok_or_else(|| invalid("Nom de fichier absent"))?;

    let target = folder.join(name);

    // If an old snapshot exists, never silently replace it.
    if fs::symlink_metadata(&target).is_ok() {
        if !fs::symlink_metadata(&target)?.is_file() || digest(&source)? != digest(&target)? {
            return Err(conflict(
                "Snapshot different : desactiver le torrent et examiner la copie",
            ));
        }
        return Ok(target);
    }

    let temporary = folder.join(format!(".copy-{}.partial", Uuid::new_v4().simple()));

    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    let _cleanup = Cleanup(temporary.clone());

    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;

    fs::copy(&source, &temporary)?;

    OpenOptions::new()
        .write(true)
        .open(&temporary)?
        .sync_all()?;

    let updated = fs::metadata(&source)?;
    if updated.len() != original.len() || updated.modified()? != original.modified()? {
        return Err(conflict("Source modifiee pendant la copie"));
    }

    if digest(&source)? != digest(&temporary)? {
        return Err(conflict("Copie differente de la source"));
    }

    if fs::symlink_metadata(&target).is_ok() {
        return Err(conflict("Snapshot concurrent detecte"));
    }

    fs::rename(&temporary, &target)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_is_separate_and_rejects_source_changes() {
        let root = std::env::temp_dir().join(format!("norylum-seed-test-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();

        let source = root.join("Test (2026).mkv");
        fs::write(&source, b"original synthetic test").unwrap();

        let id = MediaId::new("med_1234567890abcdef");

        let first = prepare_with_age(&root, &id, &source, Duration::ZERO).unwrap();

        assert_ne!(source, first);
        assert_eq!(fs::read(&source).unwrap(), fs::read(&first).unwrap());

        assert_eq!(
            first,
            prepare_with_age(&root, &id, &source, Duration::ZERO).unwrap()
        );

        fs::write(&source, b"changed").unwrap();

        assert!(prepare_with_age(&root, &id, &source, Duration::ZERO).is_err());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn seed_lock_is_exclusive() {
        let root = std::env::temp_dir().join(format!("norylum-seed-lock-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let first = SeedLock::acquire(&root).unwrap();

        assert!(SeedLock::acquire(&root).is_err());
        drop(first);

        assert!(SeedLock::acquire(&root).is_ok());
        fs::remove_dir_all(root).unwrap();
    }
}
