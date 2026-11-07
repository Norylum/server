//! Exclusive, OS-released process locks on the shared media volume.
//!
//! Two independent lock files prevent conflicting operations without forcing
//! the server and the offline metadata writer to be mutually exclusive:
//!
//!   `.norylum-instance.lock`  — held exclusively by `norylum-server`.
//!                               Prevents two server instances from sharing
//!                               the same media directory.
//!
//!   `.norylum-tagger.lock`    — held exclusively by `norylum-metadata-tags`.
//!                               Prevents two concurrent tagging operations.
//!
//! Because they use different files, the server and the tagger can run
//! simultaneously. Care must be taken not to tag a file that is actively
//! being streamed; the server will reflect updated metadata after the next
//! library scan.

use fs2::FileExt;
use std::{fs, fs::File, fs::OpenOptions, io, path::Path};

/// Held by `norylum-server` for its entire lifetime.
/// Prevents a second server instance from opening the same media volume.
pub(crate) struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    pub(crate) fn acquire(root: &Path) -> io::Result<Self> {
        acquire_exclusive(root, ".norylum-instance.lock", "Norylum media volume is already in use")
            .map(|f| Self { _file: f })
    }
}

/// Held by `norylum-metadata-tags` for the duration of a tagging run.
/// Prevents two concurrent tagging operations on the same media volume.
/// Does NOT conflict with `InstanceLock`, so the server can keep running.
pub(crate) struct TaggerLock {
    _file: File,
}

impl TaggerLock {
    pub(crate) fn acquire(root: &Path) -> io::Result<Self> {
        acquire_exclusive(root, ".norylum-tagger.lock", "A Norylum tagger is already running")
            .map(|f| Self { _file: f })
    }
}

fn acquire_exclusive(root: &Path, filename: &str, busy_msg: &str) -> io::Result<File> {
    let path = root.join(filename);
    match fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Norylum lock path is not a regular file: {filename}"),
            ));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.try_lock_exclusive().map_err(|e| {
        io::Error::new(
            io::ErrorKind::WouldBlock,
            format!("{busy_msg}: {e}"),
        )
    })?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_is_exclusive_and_released_on_drop() {
        let root =
            std::env::temp_dir().join(format!("norylum-instance-lock-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let first = InstanceLock::acquire(&root).unwrap();
        assert!(InstanceLock::acquire(&root).is_err());
        drop(first);
        let second = InstanceLock::acquire(&root).unwrap();
        drop(second);
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_lock_is_rejected() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!(
            "norylum-instance-lock-symlink-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&root).unwrap();
        symlink("/dev/null", root.join(".norylum-instance.lock")).unwrap();
        assert!(InstanceLock::acquire(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tagger_lock_is_exclusive_and_released_on_drop() {
        let root =
            std::env::temp_dir().join(format!("norylum-tagger-lock-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let first = TaggerLock::acquire(&root).unwrap();
        assert!(TaggerLock::acquire(&root).is_err());
        drop(first);
        let second = TaggerLock::acquire(&root).unwrap();
        drop(second);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn server_and_tagger_can_coexist() {
        let root =
            std::env::temp_dir().join(format!("norylum-coexist-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let server = InstanceLock::acquire(&root).unwrap();
        let tagger = TaggerLock::acquire(&root).unwrap();
        // Both locks held simultaneously — no conflict.
        drop(tagger);
        drop(server);
        fs::remove_dir_all(&root).unwrap();
    }
}
