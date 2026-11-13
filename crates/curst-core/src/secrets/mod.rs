//! secret values: the stores that hold them and the index that tracks them
//!
//! a secret is never written to a collection, environment or bundle file;
//! its value lives in a [`SecretStore`], the OS keychain or an encrypted
//! file, and `secrets-index.toml` records every key in use, since a keychain
//! cannot list its entries

pub mod detect;
mod encrypted_file;
mod host_binding;
mod index;
mod key;
mod keychain;
mod memory;
mod vault;

use std::fmt;

use zeroize::Zeroizing;

pub use encrypted_file::EncryptedFile;
pub use host_binding::{Approval, normal_origin};
pub use index::{IndexEntry, SecretIndex};
pub use key::{SecretKey, SecretScope};
pub use keychain::Keychain;
pub use memory::MemoryStore;
pub use vault::Secrets;

use crate::store::StoreError;

/// a secret value: wiped from memory when dropped, and shown as `***` by
/// `Debug` and `Display`, so it cannot reach a log or a file by accident
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    /// wraps `value`
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    /// the value itself, for the one place that needs it
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// why a secret operation failed
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// the store cannot be used here, such as a keychain on a headless
    /// Linux machine
    #[error("the secret store is not available: {0}")]
    Unavailable(String),

    /// the passphrase does not open the encrypted file
    #[error("wrong passphrase, or the secrets file is damaged")]
    WrongPassphrase,

    /// the encrypted file or the index cannot be read
    #[error("{0}")]
    Damaged(String),

    /// the value is longer than the store accepts; it is never cut short
    #[error("the secret is too long for this store: at most {limit} bytes")]
    TooLong {
        /// the limit, in bytes
        limit: u32,
    },

    /// no secret has this key
    #[error("no secret {0}")]
    NotFound(String),

    /// a URL without a scheme and host; a secret is never sent without
    /// knowing where to
    #[error("{0:?} needs a full URL, with http:// or https://, to carry a secret")]
    NoOrigin(String),

    /// a file could not be read or written
    #[error(transparent)]
    Store(#[from] StoreError),

    /// any other failure of the store
    #[error("the secret store failed: {0}")]
    Backend(String),
}

/// where secret values are kept
pub trait SecretStore: Send + Sync {
    /// the value of `key`, or `None` when it has none
    ///
    /// # Errors
    /// when the store cannot be read
    fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretError>;

    /// stores `value` under `key`, replacing any value it had
    ///
    /// # Errors
    /// when the store cannot be written or the value is too long
    fn set(&self, key: &SecretKey, value: &Secret) -> Result<(), SecretError>;

    /// stores every value, by default one after another; a store that pays
    /// for each write, like the encrypted file, writes them all at once
    ///
    /// # Errors
    /// as [`SecretStore::set`]
    fn set_all(&self, values: &[(SecretKey, Secret)]) -> Result<(), SecretError> {
        values
            .iter()
            .try_for_each(|(key, value)| self.set(key, value))
    }

    /// removes `key`; a key that has no value is not an error
    ///
    /// # Errors
    /// when the store cannot be written
    fn delete(&self, key: &SecretKey) -> Result<(), SecretError>;

    /// writes, reads back and deletes a test entry, to check the store
    /// works before it is offered
    ///
    /// # Errors
    /// whatever the store reports, or [`SecretError::Backend`] when the
    /// value read back differs
    fn probe(&self) -> Result<(), SecretError> {
        let value = Secret::new("curst probe");

        self.set(&SecretKey::Probe, &value)?;

        let back = self.get(&SecretKey::Probe)?;

        self.delete(&SecretKey::Probe)?;

        if back == Some(value) {
            Ok(())
        } else {
            Err(SecretError::Backend(
                "the probe value did not read back".into(),
            ))
        }
    }
}

/// the value of a keychain read, kept here so the keychain adapter holds no
/// logic of its own
pub(crate) fn keychain_value(
    result: Result<Vec<u8>, keyring_core::Error>,
) -> Result<Option<Secret>, SecretError> {
    match result {
        Ok(bytes) => String::from_utf8(bytes)
            .map(|value| Some(Secret::new(value)))
            .map_err(|_| SecretError::Damaged("a keychain secret is not valid UTF-8".into())),
        Err(keyring_core::Error::NoEntry) => Ok(None),
        Err(err) => Err(keychain_error(err)),
    }
}

/// the result of a keychain delete; a missing entry is fine
pub(crate) fn keychain_deleted(result: Result<(), keyring_core::Error>) -> Result<(), SecretError> {
    match result {
        Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
        Err(err) => Err(keychain_error(err)),
    }
}

pub(crate) fn keychain_error(err: keyring_core::Error) -> SecretError {
    match err {
        keyring_core::Error::TooLong(_, limit) => SecretError::TooLong { limit },
        keyring_core::Error::NoStorageAccess(err) => SecretError::Unavailable(err.to_string()),
        keyring_core::Error::NoDefaultStore => {
            SecretError::Unavailable("no keychain store is set up".into())
        }
        other => SecretError::Backend(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_never_print_their_value() {
        let secret = Secret::new("hunter2");

        assert_eq!(format!("{secret:?} {secret}"), "*** ***");
        assert_eq!(secret.expose(), "hunter2");
        assert_eq!(format!("{:?}", Some(secret)), "Some(***)");
    }

    #[test]
    fn keychain_results_are_mapped() {
        assert_eq!(
            keychain_value(Ok(b"v".to_vec())).unwrap(),
            Some(Secret::new("v"))
        );
        assert_eq!(
            keychain_value(Err(keyring_core::Error::NoEntry)).unwrap(),
            None
        );
        assert!(matches!(
            keychain_value(Ok(vec![0xff])),
            Err(SecretError::Damaged(_))
        ));
        assert!(matches!(
            keychain_value(Err(keyring_core::Error::TooLong("secret".into(), 2560))),
            Err(SecretError::TooLong { limit: 2560 })
        ));
        assert!(keychain_deleted(Ok(())).is_ok());
        assert!(keychain_deleted(Err(keyring_core::Error::NoEntry)).is_ok());
        assert!(matches!(
            keychain_deleted(Err(keyring_core::Error::NoDefaultStore)),
            Err(SecretError::Unavailable(_))
        ));
        assert!(matches!(
            keychain_error(keyring_core::Error::NoStorageAccess(Box::new(
                std::io::Error::other("locked")
            ))),
            SecretError::Unavailable(message) if message == "locked"
        ));
        assert!(matches!(
            keychain_error(keyring_core::Error::BadStoreFormat("x".into())),
            SecretError::Backend(_)
        ));
    }
}
