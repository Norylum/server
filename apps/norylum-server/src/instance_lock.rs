//! Exclusive, OS-released process lock on the shared media volume.
//! Coordinates Norylum server instances and the offline metadata writer.
//! External applications must still be managed separately.

use fs2::FileExt;
use std::{fs, fs::File, fs::OpenOptions, io, path::Path};

pub(crate) struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    pub(crate) fn acquire(root: &Path) -> io::Result<Self> {
        let path = root.join(".norylum-instance.lock");
        match fs::symlink_metadata(&path) {
            Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Norylum lock path is not a regular file",
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
                format!("Norylum media volume is already in use: {e}"),
            )
        })?;
        Ok(Self { _file: file })
    }
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
}
