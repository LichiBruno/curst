//! secrets in one file encrypted with a passphrase, using `age`
//!
//! the passphrase is held only in memory. opening the file is slow on
//! purpose (scrypt), so the last decrypted content is kept with the bytes
//! it came from and reused for as long as the file is unchanged

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use age::secrecy::SecretString;
use zeroize::Zeroizing;

use super::{Secret, SecretError, SecretKey, SecretStore};
use crate::store::fs::{read_bytes, with_lock, write_atomic};

type Values = BTreeMap<String, Secret>;

/// scrypt's cost as a power of two, fixed rather than measured on each
/// machine, so a file written on a fast machine opens on a slow one
const WORK_FACTOR: u8 = 18;

/// the highest cost accepted when opening a file: enough for files written
/// by curst, low enough that a hostile file cannot take minutes
const MAX_WORK_FACTOR: u8 = 20;

/// the file's bytes and what they decrypt to
#[derive(Default)]
struct Cache {
    bytes: Vec<u8>,
    values: Values,
}

/// secrets kept in `secrets.age`, encrypted with a passphrase
pub struct EncryptedFile {
    path: PathBuf,
    passphrase: Secret,

    /// scrypt's cost as a power of two
    work_factor: u8,
    cache: Mutex<Option<Cache>>,
}

impl std::fmt::Debug for EncryptedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncryptedFile")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl EncryptedFile {
    /// the file at `path`, opened with `passphrase`; nothing is read until
    /// the first secret is needed
    pub fn new(path: impl Into<PathBuf>, passphrase: Secret) -> Self {
        Self {
            path: path.into(),
            passphrase,
            work_factor: WORK_FACTOR,
            cache: Mutex::new(None),
        }
    }

    /// a cheaper scrypt cost, so tests stay fast
    #[cfg(test)]
    pub(crate) fn with_work_factor(mut self, log_n: u8) -> Self {
        self.work_factor = log_n;

        self
    }

    /// the file
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn secret_string(&self) -> SecretString {
        SecretString::from(self.passphrase.expose().to_owned())
    }

    /// every value in the file; an absent file holds none, but a file that
    /// cannot be opened is an error, never an empty store
    fn values(&self) -> Result<Values, SecretError> {
        let Some(bytes) = read_bytes(&self.path)? else {
            return Ok(Values::new());
        };

        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);

        if let Some(cached) = cache.as_ref().filter(|c| c.bytes == bytes) {
            return Ok(cached.values.clone());
        }

        let mut identity = age::scrypt::Identity::new(self.secret_string());

        identity.set_max_work_factor(MAX_WORK_FACTOR);

        let plain = Zeroizing::new(age::decrypt(&identity, &bytes).map_err(|e| match e {
            age::DecryptError::DecryptionFailed | age::DecryptError::NoMatchingKeys => {
                SecretError::WrongPassphrase
            }
            age::DecryptError::ExcessiveWork { .. } => SecretError::Damaged(format!(
                "{} asks for more work to open than curst allows",
                self.path.display()
            )),
            other => SecretError::Damaged(format!("{}: {other}", self.path.display())),
        })?);

        // the parser's message may quote the decrypted text, so it is never
        // passed on
        let parsed: BTreeMap<String, String> = serde_json::from_slice(&plain).map_err(|_| {
            SecretError::Damaged(format!(
                "{} does not hold secrets in the expected form",
                self.path.display()
            ))
        })?;
        let values: Values = parsed
            .into_iter()
            .map(|(key, value)| (key, Secret::new(value)))
            .collect();

        *cache = Some(Cache {
            bytes,
            values: values.clone(),
        });

        Ok(values)
    }

    fn write(&self, values: Values) -> Result<(), SecretError> {
        let plain: BTreeMap<&str, &str> = values
            .iter()
            .map(|(key, value)| (key.as_str(), value.expose()))
            .collect();
        let plain = Zeroizing::new(
            serde_json::to_vec(&plain).map_err(|e| SecretError::Backend(e.to_string()))?,
        );
        let mut recipient = age::scrypt::Recipient::new(self.secret_string());

        recipient.set_work_factor(self.work_factor);

        let bytes =
            age::encrypt(&recipient, &plain).map_err(|e| SecretError::Backend(e.to_string()))?;

        write_atomic(&self.path, &bytes)?;

        *self.cache.lock().unwrap_or_else(PoisonError::into_inner) = Some(Cache { bytes, values });

        Ok(())
    }

    /// reads, changes and writes the file under its lock, re-reading it
    /// first in case another process changed it
    fn change(&self, change: impl FnOnce(&mut Values)) -> Result<(), SecretError> {
        with_lock(&self.path, || {
            let mut values = self.values()?;

            change(&mut values);
            self.write(values)
        })
    }
}

impl SecretStore for EncryptedFile {
    fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretError> {
        Ok(self.values()?.remove(&key.to_string()))
    }

    fn set(&self, key: &SecretKey, value: &Secret) -> Result<(), SecretError> {
        self.change(|values| {
            values.insert(key.to_string(), value.clone());
        })
    }

    fn set_all(&self, values: &[(SecretKey, Secret)]) -> Result<(), SecretError> {
        self.change(|stored| {
            for (key, value) in values {
                stored.insert(key.to_string(), value.clone());
            }
        })
    }

    fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        if self.get(key)?.is_none() {
            return Ok(());
        }

        self.change(|values| {
            values.remove(&key.to_string());
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vars::VarName;

    fn key(name: &str) -> SecretKey {
        SecretKey::Global {
            name: VarName::new(name).unwrap(),
        }
    }

    fn file(path: &Path, passphrase: &str) -> EncryptedFile {
        EncryptedFile::new(path, Secret::new(passphrase)).with_work_factor(2)
    }

    #[test]
    fn values_survive_a_new_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.age");
        let store = file(&path, "pass");

        assert_eq!(store.get(&key("a")).unwrap(), None);

        store.set(&key("a"), &Secret::new("1")).unwrap();
        store
            .set_all(&[(key("b"), Secret::new("2")), (key("c"), Secret::new("3"))])
            .unwrap();
        store.delete(&key("b")).unwrap();

        assert_eq!(store.get(&key("c")).unwrap(), Some(Secret::new("3")));
        store.delete(&key("missing")).unwrap();

        let again = file(&path, "pass");

        assert_eq!(again.get(&key("a")).unwrap(), Some(Secret::new("1")));
        assert_eq!(again.get(&key("b")).unwrap(), None);
        assert!(again.probe().is_ok());
        assert_eq!(again.path(), path);
        assert!(!format!("{again:?}").contains("pass"));

        let text = std::fs::read(&path).unwrap();

        assert!(!String::from_utf8_lossy(&text).contains("curst/global/a"));
    }

    #[test]
    fn a_wrong_passphrase_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.age");

        file(&path, "right")
            .set(&key("a"), &Secret::new("1"))
            .unwrap();

        let before = std::fs::read(&path).unwrap();
        let wrong = file(&path, "wrong");

        assert!(matches!(
            wrong.get(&key("a")),
            Err(SecretError::WrongPassphrase)
        ));
        assert!(matches!(
            wrong.set(&key("b"), &Secret::new("2")),
            Err(SecretError::WrongPassphrase)
        ));
        assert!(matches!(
            wrong.delete(&key("a")),
            Err(SecretError::WrongPassphrase)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn a_damaged_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.age");

        std::fs::write(&path, "not age").unwrap();

        assert!(matches!(
            file(&path, "p").get(&key("a")),
            Err(SecretError::Damaged(_))
        ));

        // a valid age file whose content is not the expected map
        let recipient = {
            let mut r = age::scrypt::Recipient::new(SecretString::from("p".to_owned()));

            r.set_work_factor(2);
            r
        };

        std::fs::write(
            &path,
            age::encrypt(&recipient, b"\"leaked-value\"").unwrap(),
        )
        .unwrap();

        let err = file(&path, "p").get(&key("a")).unwrap_err();

        assert!(matches!(err, SecretError::Damaged(_)));
        assert!(!err.to_string().contains("leaked-value"), "{err}");

        // a cost above the limit is refused with its own message, read from
        // the header before any key is derived; the cheap file is made to
        // claim a higher cost
        let cheap = String::from_utf8_lossy(&age::encrypt(&recipient, b"{}").unwrap()).into_owned();
        let costly = cheap.replacen(" 2\n", &format!(" {}\n", MAX_WORK_FACTOR + 1), 1);

        assert_ne!(cheap, costly);

        std::fs::write(&path, costly.as_bytes()).unwrap();

        let err = file(&path, "p").get(&key("a")).unwrap_err();

        assert!(err.to_string().contains("more work"), "{err}");
    }

    #[test]
    fn another_writer_is_noticed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.age");
        let one = file(&path, "p");
        let two = file(&path, "p");

        one.set(&key("a"), &Secret::new("1")).unwrap();
        two.set(&key("b"), &Secret::new("2")).unwrap();
        one.set(&key("c"), &Secret::new("3")).unwrap();

        for name in ["a", "b", "c"] {
            assert!(two.get(&key(name)).unwrap().is_some(), "{name}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.age");

        file(&path, "p").set(&key("a"), &Secret::new("1")).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();

        assert_eq!(mode & 0o777, 0o600);
    }
}
