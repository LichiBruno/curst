//! `config.toml`, the user's settings
//!
//! reading never stops over one bad value: it is reported, and the default
//! takes its place. writes change one value and keep the rest of the file,
//! comments and layout included
//!
//! ```toml
//! theme = "dark"
//! mouse = true
//!
//! [keys]
//! preset = "standard"
//!
//! [keys.bindings]
//! "global.send" = "ctrl+r"
//! "sidebar.delete" = ["d", "delete"]
//!
//! [http]
//! timeout = 30
//! max_body_mb = 10
//!
//! [secrets]
//! backend = "keychain"
//! ```

use std::path::Path;

use toml_edit::{DocumentMut, Item, Table, Value};

use super::StoreError;
use super::fs::{read_regular, write_atomic};

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

/// the settings curst reads from `config.toml`; `None` means the default
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// the chosen secrets backend, `None` until the user picks one
    pub secrets_backend: Option<Backend>,

    /// the theme: a built-in name, or a file in the themes folder
    pub theme: Option<String>,

    /// whether clicks and the wheel work; on by default
    pub mouse: Option<bool>,

    /// the keymap preset, such as `standard` or `vim`
    pub keys_preset: Option<String>,

    /// keys for actions, as `"layer.action"` and its keys; an empty list
    /// unbinds the action
    pub key_bindings: Vec<(String, Vec<String>)>,

    /// seconds a request may take, 0 for no limit
    pub http_timeout: Option<u64>,

    /// how many megabytes of a response body are kept in memory
    pub http_max_body_mb: Option<u64>,
}

/// the known top-level keys and sections
const KNOWN: [&str; 5] = ["theme", "mouse", "keys", "http", "secrets"];

fn read_document(path: &Path) -> Result<DocumentMut, StoreError> {
    match read_regular(path)? {
        Some(text) => text
            .parse::<DocumentMut>()
            .map_err(|e| StoreError::parse(path, e.message())),
        None => Ok(DocumentMut::new()),
    }
}

fn string(item: Option<&Item>, what: &str, warnings: &mut Vec<String>) -> Option<String> {
    let item = item?;

    if let Some(text) = item.as_str() {
        return Some(text.to_owned());
    }

    warnings.push(format!("{what} must be text"));

    None
}

fn flag(item: Option<&Item>, what: &str, warnings: &mut Vec<String>) -> Option<bool> {
    let item = item?;

    if let Some(on) = item.as_bool() {
        return Some(on);
    }

    warnings.push(format!("{what} must be true or false"));

    None
}

fn number(item: Option<&Item>, what: &str, least: u64, warnings: &mut Vec<String>) -> Option<u64> {
    let item = item?;

    match item.as_integer().and_then(|n| u64::try_from(n).ok()) {
        Some(n) if n >= least => Some(n),
        _ => {
            warnings.push(format!("{what} must be a whole number of at least {least}"));

            None
        }
    }
}

fn keys(item: &Item) -> Option<Vec<String>> {
    if let Some(text) = item.as_str() {
        return Some(vec![text.to_owned()]);
    }

    item.as_array()?
        .iter()
        .map(|v| v.as_str().map(ToOwned::to_owned))
        .collect()
}

impl Settings {
    /// the settings in `path`, and a warning for each value that was not
    /// used; a missing file means defaults
    ///
    /// # Errors
    /// when the file cannot be read or is not TOML at all
    pub fn load(path: &Path) -> Result<(Self, Vec<String>), StoreError> {
        let document = read_document(path)?;
        let root = document.as_table();
        let mut warnings = Vec::new();
        let mut settings = Self::default();

        for (key, _) in root {
            if !KNOWN.contains(&key) {
                warnings.push(format!("unknown setting {key:?}"));
            }
        }

        settings.theme = string(root.get("theme"), "theme", &mut warnings);
        settings.mouse = flag(root.get("mouse"), "mouse", &mut warnings);

        let section = |name: &str| root.get(name).and_then(Item::as_table_like);

        if let Some(keys_section) = section("keys") {
            settings.keys_preset = string(keys_section.get("preset"), "keys.preset", &mut warnings);

            if let Some(bindings) = keys_section.get("bindings").and_then(Item::as_table_like) {
                for (name, item) in bindings.iter() {
                    match keys(item) {
                        Some(list) => settings.key_bindings.push((name.to_owned(), list)),
                        None => warnings.push(format!(
                            "keys.bindings.{name} must be a key or a list of keys"
                        )),
                    }
                }
            }
        }

        if let Some(http) = section("http") {
            settings.http_timeout = number(http.get("timeout"), "http.timeout", 0, &mut warnings);
            settings.http_max_body_mb = number(
                http.get("max_body_mb"),
                "http.max_body_mb",
                1,
                &mut warnings,
            );
        }

        if let Some(secrets) = section("secrets") {
            settings.secrets_backend =
                match string(secrets.get("backend"), "secrets.backend", &mut warnings).as_deref() {
                    None => None,
                    Some("keychain") => Some(Backend::Keychain),
                    Some("encrypted-file") => Some(Backend::EncryptedFile),
                    Some(other) => {
                        warnings.push(format!(
                        "secrets.backend must be \"keychain\" or \"encrypted-file\", not {other:?}"
                    ));

                        None
                    }
                };
        }

        Ok((settings, warnings))
    }

    /// sets one value, `None` removes it; everything else in the file stays
    fn set(path: &Path, keys: &[&str], value: Option<Value>) -> Result<(), StoreError> {
        let mut document = read_document(path)?;
        let Some((last, sections)) = keys.split_last() else {
            return Ok(());
        };

        let mut table: &mut Table = document.as_table_mut();

        for section in sections {
            let item = table
                .entry(section)
                .or_insert_with(|| Item::Table(Table::new()));

            if !item.is_table() {
                *item = Item::Table(Table::new());
            }

            let Some(next) = item.as_table_mut() else {
                return Ok(());
            };

            table = next;
        }

        match (value, table.get_mut(last)) {
            // replaced in place, so the comments around the old value stay
            (Some(mut value), Some(item)) if item.is_value() => {
                if let Some(old) = item.as_value() {
                    *value.decor_mut() = old.decor().clone();
                }

                *item = Item::Value(value);
            }
            (Some(value), _) => {
                table.insert(last, Item::Value(value));
            }
            (None, _) => {
                table.remove(last);
            }
        }

        write_atomic(path, document.to_string().as_bytes())
    }

    /// records the secrets backend
    ///
    /// # Errors
    /// when the file cannot be read, parsed or written
    pub fn set_backend(path: &Path, backend: Option<Backend>) -> Result<(), StoreError> {
        Self::set(
            path,
            &["secrets", "backend"],
            backend.map(|b| b.as_str().into()),
        )
    }

    /// records the theme
    ///
    /// # Errors
    /// when the file cannot be read, parsed or written
    pub fn set_theme(path: &Path, theme: &str) -> Result<(), StoreError> {
        Self::set(path, &["theme"], Some(theme.into()))
    }

    /// records the keymap preset
    ///
    /// # Errors
    /// when the file cannot be read, parsed or written
    pub fn set_keys_preset(path: &Path, preset: &str) -> Result<(), StoreError> {
        Self::set(path, &["keys", "preset"], Some(preset.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn every_setting_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        assert_eq!(
            Settings::load(&path).unwrap(),
            (Settings::default(), Vec::new())
        );

        write(
            &path,
            r#"theme = "light"
mouse = false

[keys]
preset = "vim"

[keys.bindings]
"global.send" = "ctrl+enter"
"sidebar.delete" = ["d", "delete"]
"request.make-secret" = []

[http]
timeout = 0
max_body_mb = 50

[secrets]
backend = "encrypted-file"
"#,
        );

        let (settings, warnings) = Settings::load(&path).unwrap();

        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(
            settings,
            Settings {
                secrets_backend: Some(Backend::EncryptedFile),
                theme: Some("light".into()),
                mouse: Some(false),
                keys_preset: Some("vim".into()),
                key_bindings: vec![
                    ("global.send".into(), vec!["ctrl+enter".into()]),
                    ("sidebar.delete".into(), vec!["d".into(), "delete".into()]),
                    ("request.make-secret".into(), vec![]),
                ],
                http_timeout: Some(0),
                http_max_body_mb: Some(50),
            }
        );
    }

    #[test]
    fn bad_values_are_reported_and_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        write(
            &path,
            r#"theme = 3
mouse = "yes"
color = "red"

[keys]
preset = true

[keys.bindings]
"global.send" = 5
"global.save" = "ctrl+s"

[http]
timeout = -1
max_body_mb = 0

[secrets]
backend = "vault"
"#,
        );

        let (settings, warnings) = Settings::load(&path).unwrap();

        assert_eq!(
            settings,
            Settings {
                key_bindings: vec![("global.save".into(), vec!["ctrl+s".into()])],
                ..Settings::default()
            }
        );
        assert_eq!(warnings.len(), 8, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("\"color\"")));
        assert!(warnings.iter().any(|w| w.contains("vault")));

        write(&path, "[secrets\n");

        assert!(Settings::load(&path).is_err());
        assert!(Settings::set_theme(&path, "dark").is_err());
    }

    #[test]
    fn a_change_keeps_the_rest_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = "# my settings\ntheme = \"light\" # bright\n\n[keys]\npreset = \"vim\"\n";

        write(&path, original);
        Settings::set_backend(&path, Some(Backend::Keychain)).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();

        assert!(text.starts_with(original), "{text}");
        assert!(text.contains("[secrets]\nbackend = \"keychain\""), "{text}");

        Settings::set_backend(&path, None).unwrap();
        Settings::set_theme(&path, "dark").unwrap();
        Settings::set_keys_preset(&path, "standard").unwrap();

        let (settings, _) = Settings::load(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();

        assert_eq!(settings.theme.as_deref(), Some("dark"));
        assert_eq!(settings.keys_preset.as_deref(), Some("standard"));
        assert_eq!(settings.secrets_backend, None);
        assert!(text.contains("# my settings"), "{text}");
        assert!(text.contains("# bright"), "{text}");

        // a value where a section should be is replaced by the section
        write(&path, "secrets = 1\n");
        Settings::set_backend(&path, Some(Backend::EncryptedFile)).unwrap();

        assert_eq!(
            Settings::load(&path).unwrap().0.secrets_backend,
            Some(Backend::EncryptedFile)
        );
    }
}
