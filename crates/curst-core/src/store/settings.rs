//! `config.toml`, the user's settings
//!
//! only the secrets backend is kept here so far; other sections are left
//! as they are when the file is saved

use std::path::Path;

use super::StoreError;
use super::fs::{read_optional, write_atomic};

/// where secret values are kept
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// the OS keychain
    Keychain,

    /// `secrets.age`, encrypted with a passphrase
    EncryptedFile,
}

impl Backend {
    fn as_str(self) -> &'static str {
        match self {
            Self::Keychain => "keychain",
            Self::EncryptedFile => "encrypted-file",
        }
    }
}

/// the settings curst reads from `config.toml`
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// the chosen secrets backend, `None` until the user picks one
    pub secrets_backend: Option<Backend>,
}

fn read_table(path: &Path) -> Result<toml::Table, StoreError> {
    match read_optional(path)? {
        Some(text) => text
            .parse::<toml::Table>()
            .map_err(|e| StoreError::parse(path, e.message())),
        None => Ok(toml::Table::new()),
    }
}

impl Settings {
    /// the settings in `path`; a missing file means defaults
    ///
    /// # Errors
    /// when the file cannot be read, is not TOML, or names an unknown
    /// backend
    pub fn load(path: &Path) -> Result<Self, StoreError> {
        let table = read_table(path)?;
        let backend = table
            .get("secrets")
            .and_then(|s| s.get("backend"))
            .map(|value| match value.as_str() {
                Some("keychain") => Ok(Backend::Keychain),
                Some("encrypted-file") => Ok(Backend::EncryptedFile),
                _ => Err(StoreError::parse(
                    path,
                    format!(
                        "secrets.backend must be \"keychain\" or \"encrypted-file\", not {value}"
                    ),
                )),
            })
            .transpose()?;

        Ok(Self {
            secrets_backend: backend,
        })
    }

    /// writes the settings to `path`, keeping every other section
    ///
    /// # Errors
    /// when the file cannot be read or written
    pub fn save(&self, path: &Path) -> Result<(), StoreError> {
        let mut table = read_table(path)?;
        let secrets = table
            .entry("secrets")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));

        if let toml::Value::Table(secrets) = secrets {
            match self.secrets_backend {
                Some(backend) => {
                    secrets.insert("backend".into(), backend.as_str().into());
                }
                None => {
                    secrets.remove("backend");
                }
            }
        }

        let text = toml::to_string_pretty(&table).unwrap_or_default();

        write_atomic(path, text.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backend_round_trips_and_other_settings_stay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        assert_eq!(Settings::load(&path).unwrap(), Settings::default());

        std::fs::write(&path, "[theme]\nname = \"dark\"\n").unwrap();

        for backend in [Backend::Keychain, Backend::EncryptedFile] {
            let settings = Settings {
                secrets_backend: Some(backend),
            };

            settings.save(&path).unwrap();

            assert_eq!(Settings::load(&path).unwrap(), settings);
        }

        Settings::default().save(&path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();

        assert!(text.contains("[theme]"));
        assert!(!text.contains("backend"));
        assert_eq!(Settings::load(&path).unwrap(), Settings::default());
    }

    #[test]
    fn bad_settings_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        std::fs::write(&path, "[secrets]\nbackend = \"vault\"\n").unwrap();

        assert!(matches!(
            Settings::load(&path),
            Err(StoreError::Parse { .. })
        ));

        std::fs::write(&path, "[secrets\n").unwrap();

        assert!(Settings::load(&path).is_err());
        assert!(Settings::default().save(&path).is_err());
    }
}
