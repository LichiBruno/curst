//! `secrets-index.toml`: every secret key in use and the origins it is
//! approved for, never a value
//!
//! OS keychains cannot list their entries, so this file is what migration
//! and cleanup read. a stored secret always has an entry here: entries are
//! added before a value is stored and removed after it is deleted

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{SecretError, SecretKey};
use crate::store::fs::{read_optional, with_lock, write_atomic};

/// one secret in use
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    /// its key
    pub key: SecretKey,

    /// the origins it may be sent to, as `https://host:port`
    pub hosts: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct IndexFile {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    secrets: Vec<EntryFile>,
}

#[derive(Debug, Serialize, Deserialize)]
struct EntryFile {
    key: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    hosts: Vec<String>,
}

/// the index file
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretIndex {
    file: PathBuf,
}

/// the index while its lock is held
pub(crate) struct Locked<'a> {
    index: &'a SecretIndex,
    pub(crate) entries: Vec<IndexEntry>,
}

impl Locked<'_> {
    pub(crate) fn contains(&self, key: &SecretKey) -> bool {
        self.entries.iter().any(|e| e.key == *key)
    }

    pub(crate) fn save(&self) -> Result<(), SecretError> {
        self.index.write(&self.entries)
    }
}

impl SecretIndex {
    /// the index stored in `file`
    pub fn new(file: impl Into<PathBuf>) -> Self {
        Self { file: file.into() }
    }

    /// every entry
    ///
    /// # Errors
    /// when the file cannot be read, or any entry in it is not a valid key:
    /// a broken index is never skipped over, or a secret could lose its
    /// entry on the next write
    pub fn entries(&self) -> Result<Vec<IndexEntry>, SecretError> {
        let Some(text) = read_optional(&self.file)? else {
            return Ok(Vec::new());
        };

        let damaged =
            |message: String| SecretError::Damaged(format!("{}: {message}", self.file.display()));
        let file: IndexFile = toml::from_str(&text).map_err(|e| damaged(e.message().to_owned()))?;

        file.secrets
            .into_iter()
            .map(|e| {
                let key = e
                    .key
                    .parse()
                    .map_err(|err: super::key::BadKey| damaged(err.to_string()))?;

                Ok(IndexEntry {
                    key,
                    hosts: e.hosts,
                })
            })
            .collect()
    }

    fn write(&self, entries: &[IndexEntry]) -> Result<(), SecretError> {
        let file = IndexFile {
            secrets: entries
                .iter()
                .map(|e| EntryFile {
                    key: e.key.to_string(),
                    hosts: e.hosts.clone(),
                })
                .collect(),
        };

        write_atomic(&self.file, crate::store::format_toml(&file).as_bytes())?;

        Ok(())
    }

    /// runs `f` with the entries while holding the index lock, which also
    /// covers the store operations `f` makes
    pub(crate) fn locked<T>(
        &self,
        f: impl FnOnce(&mut Locked<'_>) -> Result<T, SecretError>,
    ) -> Result<T, SecretError> {
        with_lock::<T, SecretError>(&self.file, || {
            let mut locked = Locked {
                index: self,
                entries: self.entries()?,
            };

            f(&mut locked)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_broken_index_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let index = SecretIndex::new(dir.path().join("secrets-index.toml"));

        assert_eq!(index.entries().unwrap(), Vec::new());

        std::fs::write(&index.file, "secrets = 1").unwrap();

        assert!(matches!(index.entries(), Err(SecretError::Damaged(_))));

        std::fs::write(&index.file, "[[secrets]]\nkey = \"nope\"\n").unwrap();

        assert!(matches!(index.entries(), Err(SecretError::Damaged(_))));
        assert!(index.locked(|_| Ok(())).is_err());
    }

    #[test]
    fn the_index_file_is_readable_toml() {
        let dir = tempfile::tempdir().unwrap();
        let index = SecretIndex::new(dir.path().join("secrets-index.toml"));
        let key: SecretKey = "curst/global/token".parse().unwrap();

        index
            .locked(|locked| {
                locked.entries.push(IndexEntry {
                    key: key.clone(),
                    hosts: vec!["https://api.example.com:443".into()],
                });

                assert!(locked.contains(&key));

                locked.save()
            })
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(&index.file).unwrap(),
            "[[secrets]]\nkey = \"curst/global/token\"\nhosts = [\"https://api.example.com:443\"]\n"
        );
        assert_eq!(index.entries().unwrap()[0].key, key);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = std::fs::metadata(&index.file).unwrap().permissions().mode();

            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
