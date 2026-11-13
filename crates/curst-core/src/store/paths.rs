//! where curst keeps its configuration and data

use std::ffi::OsString;
use std::path::PathBuf;

use etcetera::BaseStrategy;

use super::StoreError;

/// the configuration and data directories
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `config.toml` and themes
    pub config_dir: PathBuf,

    /// collections, the registry and global variables
    pub data_dir: PathBuf,
}

impl Paths {
    /// the directories for this user: `CURST_CONFIG_DIR` and
    /// `CURST_DATA_DIR` when set, otherwise the XDG directories on macOS and
    /// Linux and `%APPDATA%` on Windows, each with a `curst` folder
    ///
    /// # Errors
    /// [`StoreError::NoHome`] when no home directory can be found and no
    /// override is set
    pub fn from_env() -> Result<Self, StoreError> {
        Self::resolve(
            |key| std::env::var_os(key),
            || {
                let base = etcetera::choose_base_strategy().map_err(|_| StoreError::NoHome)?;

                Ok((base.config_dir(), base.data_dir()))
            },
        )
    }

    /// both directories under one root, for tests and portable setups
    #[must_use]
    pub fn under(root: impl Into<PathBuf>) -> Self {
        let root = root.into();

        Self {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
        }
    }

    fn resolve(
        env: impl Fn(&str) -> Option<OsString>,
        base: impl FnOnce() -> Result<(PathBuf, PathBuf), StoreError>,
    ) -> Result<Self, StoreError> {
        let set = |key| env(key).filter(|v| !v.is_empty()).map(PathBuf::from);

        if let (Some(config_dir), Some(data_dir)) = (set("CURST_CONFIG_DIR"), set("CURST_DATA_DIR"))
        {
            return Ok(Self {
                config_dir,
                data_dir,
            });
        }

        let (config, data) = base()?;

        Ok(Self {
            config_dir: set("CURST_CONFIG_DIR").unwrap_or_else(|| config.join("curst")),
            data_dir: set("CURST_DATA_DIR").unwrap_or_else(|| data.join("curst")),
        })
    }

    /// the user settings file
    #[must_use]
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    /// the folder holding one folder per collection
    #[must_use]
    pub fn collections_dir(&self) -> PathBuf {
        self.data_dir.join("collections")
    }

    /// the list of known collections and their local ids
    #[must_use]
    pub fn registry_file(&self) -> PathBuf {
        self.data_dir.join("registry.toml")
    }

    /// variables shared by every collection
    #[must_use]
    pub fn globals_file(&self) -> PathBuf {
        self.data_dir.join("globals.toml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::unnecessary_wraps)]
    fn defaults() -> Result<(PathBuf, PathBuf), StoreError> {
        Ok((
            PathBuf::from("/home/u/.config"),
            PathBuf::from("/home/u/.local/share"),
        ))
    }

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn defaults_get_a_curst_folder() {
        let paths = Paths::resolve(env(&[]), defaults).unwrap();

        assert_eq!(paths.config_dir, PathBuf::from("/home/u/.config/curst"));
        assert_eq!(paths.data_dir, PathBuf::from("/home/u/.local/share/curst"));
        assert_eq!(
            paths.config_file(),
            PathBuf::from("/home/u/.config/curst/config.toml")
        );
        assert_eq!(
            paths.collections_dir(),
            PathBuf::from("/home/u/.local/share/curst/collections")
        );
        assert_eq!(
            paths.registry_file(),
            PathBuf::from("/home/u/.local/share/curst/registry.toml")
        );
        assert_eq!(
            paths.globals_file(),
            PathBuf::from("/home/u/.local/share/curst/globals.toml")
        );
    }

    #[test]
    fn overrides_win_one_by_one() {
        let paths = Paths::resolve(
            env(&[("CURST_DATA_DIR", "/d"), ("CURST_CONFIG_DIR", "")]),
            defaults,
        )
        .unwrap();

        assert_eq!(paths.data_dir, PathBuf::from("/d"));
        assert_eq!(paths.config_dir, PathBuf::from("/home/u/.config/curst"));
    }

    #[test]
    fn both_overrides_need_no_home() {
        let paths = Paths::resolve(
            env(&[("CURST_DATA_DIR", "/d"), ("CURST_CONFIG_DIR", "/c")]),
            || Err(StoreError::NoHome),
        )
        .unwrap();

        assert_eq!(
            paths,
            Paths {
                config_dir: "/c".into(),
                data_dir: "/d".into()
            }
        );
        assert!(matches!(
            Paths::resolve(env(&[]), || Err(StoreError::NoHome)),
            Err(StoreError::NoHome)
        ));
    }

    #[test]
    fn under_one_root() {
        let paths = Paths::under("/r");

        assert_eq!(paths.config_dir, PathBuf::from("/r/config"));
        assert_eq!(paths.data_dir, PathBuf::from("/r/data"));
    }

    // pins the XDG choice on macOS and Linux: the Apple folders under
    // ~/Library must never be picked
    #[cfg(unix)]
    #[test]
    fn the_real_environment_uses_xdg() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let expected = |var: &str, fallback: &str| {
            std::env::var_os(var)
                .filter(|v| !v.is_empty())
                .map_or_else(|| home.join(fallback), PathBuf::from)
        };
        let base = etcetera::choose_base_strategy().unwrap();

        assert_eq!(base.config_dir(), expected("XDG_CONFIG_HOME", ".config"));
        assert_eq!(base.data_dir(), expected("XDG_DATA_HOME", ".local/share"));
    }
}
