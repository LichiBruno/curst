//! atomic writes and lock files

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::StoreError;

/// the prefix of temporary files; loaders skip names that start with a dot
pub(crate) const TEMP_PREFIX: &str = ".curst-";

/// writes `bytes` to `path` through a temporary file in the same folder
/// and a rename, so a reader sees the old file or the new one, never half
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));

    fs::create_dir_all(dir).map_err(|e| StoreError::io(dir, e))?;

    let mut file = tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .tempfile_in(dir)
        .map_err(|e| StoreError::io(dir, e))?;

    file.write_all(bytes)
        .and_then(|()| file.as_file().sync_all())
        .map_err(|e| StoreError::io(file.path(), e))?;
    file.persist(path)
        .map_err(|e| StoreError::io(path, e.error))?;

    Ok(())
}

/// the lock file guarding `path`; the data file itself cannot be locked
/// because every write replaces it
fn lock_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_owned();

    name.push(".lock");

    path.with_file_name(name)
}

/// runs `f` while holding the lock for `path`, which other curst processes
/// wait for; the lock file is left in place for the next writer
pub(crate) fn with_lock<T>(
    path: &Path,
    f: impl FnOnce() -> Result<T, StoreError>,
) -> Result<T, StoreError> {
    let lock = lock_path(path);

    if let Some(dir) = lock.parent() {
        fs::create_dir_all(dir).map_err(|e| StoreError::io(dir, e))?;
    }

    let file: File = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock)
        .map_err(|e| StoreError::io(&lock, e))?;

    file.lock().map_err(|e| StoreError::io(&lock, e))?;

    // the lock is released when the file closes, also on an early return
    f()
}

/// the file's text, or `None` when it does not exist
pub(crate) fn read_optional(path: &Path) -> Result<Option<String>, StoreError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(StoreError::io(path, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_creates_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/file.toml");

        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn atomic_write_into_a_file_fails() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");

        fs::write(&blocker, "x").unwrap();

        assert!(write_atomic(&blocker.join("inner.toml"), b"x").is_err());
    }

    #[test]
    fn the_lock_is_a_sidecar_and_is_released() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.toml");

        assert_eq!(lock_path(&path), dir.path().join("registry.toml.lock"));

        let inside = with_lock(&path, || {
            let other = File::open(lock_path(&path)).unwrap();

            Ok(other.try_lock().is_err())
        })
        .unwrap();

        assert!(inside);
        assert!(File::open(lock_path(&path)).unwrap().try_lock().is_ok());
    }

    #[test]
    fn errors_inside_the_lock_pass_through() {
        let dir = tempfile::tempdir().unwrap();
        let result: Result<(), _> = with_lock(&dir.path().join("x"), || Err(StoreError::NoHome));

        assert!(matches!(result, Err(StoreError::NoHome)));
    }

    #[test]
    fn optional_reads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a");

        assert_eq!(read_optional(&path).unwrap(), None);

        fs::write(&path, "x").unwrap();

        assert_eq!(read_optional(&path).unwrap().as_deref(), Some("x"));
        assert!(read_optional(dir.path()).is_err());
    }
}
