//! the OS keychain: macOS Keychain, Windows Credential Manager or the
//! Secret Service on Linux
//!
//! a thin adapter with no logic of its own: CI cannot run a keychain, so
//! this file is left out of the coverage figure and every decision about
//! results lives in the parent module

use std::sync::Arc;

use keyring_core::Entry;

use super::{
    Secret, SecretError, SecretKey, SecretStore, keychain_deleted, keychain_error, keychain_value,
};

/// the service name every curst entry is filed under
const SERVICE: &str = "curst";

/// the OS keychain of this platform
#[derive(Debug, Clone, Copy)]
pub struct Keychain(());

impl Keychain {
    /// connects to the keychain of this platform
    ///
    /// # Errors
    /// [`SecretError::Unavailable`] when the platform has none, such as
    /// Linux without a Secret Service
    pub fn open() -> Result<Self, SecretError> {
        #[cfg(target_os = "macos")]
        let store = apple_native_keyring_store::keychain::Store::new();

        #[cfg(target_os = "windows")]
        let store = windows_native_keyring_store::Store::new();

        #[cfg(all(unix, not(target_os = "macos")))]
        let store = zbus_secret_service_keyring_store::Store::new();

        let store: Arc<keyring_core::CredentialStore> = store.map_err(keychain_error)?;

        keyring_core::set_default_store(store);

        Ok(Self(()))
    }

    fn entry(key: &SecretKey) -> Result<Entry, SecretError> {
        Entry::new(SERVICE, &key.to_string()).map_err(keychain_error)
    }
}

impl SecretStore for Keychain {
    fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretError> {
        keychain_value(Self::entry(key)?.get_secret())
    }

    // UTF-8 bytes, not a password: Windows stores passwords as UTF-16,
    // which halves its size limit
    fn set(&self, key: &SecretKey, value: &Secret) -> Result<(), SecretError> {
        Self::entry(key)?
            .set_secret(value.expose().as_bytes())
            .map_err(keychain_error)
    }

    fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        keychain_deleted(Self::entry(key)?.delete_credential())
    }
}
