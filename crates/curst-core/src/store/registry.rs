//! the local list of known collections: where each one is, its local id,
//! whether it is trusted and which environment is active
//!
//! only this machine reads it, so per-user state lives here and never in a
//! collection folder

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::format::to_toml;
use super::fs::{read_optional, with_lock, write_atomic};
use super::{StoreError, ids};
use crate::model::{EnvId, LocalId};

/// one known collection
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// the collection folder, canonical
    pub path: PathBuf,

    /// the local id its secrets are stored under
    pub id: LocalId,

    /// whether its secrets may be used and its requests sent
    pub trusted: bool,

    /// the environment chosen last
    pub active_environment: Option<EnvId>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryFile {
    #[serde(default, rename = "collections", skip_serializing_if = "Vec::is_empty")]
    entries: Vec<EntryFile>,
}

#[derive(Debug, Serialize, Deserialize)]
struct EntryFile {
    path: PathBuf,
    id: String,

    #[serde(default)]
    trusted: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_environment: Option<String>,
}

/// `registry.toml`, read freely and changed only under its lock
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    file: PathBuf,
}

/// the canonical form of `path`, also for a path that no longer exists,
/// so entries match however the path was spelled
fn canonical(path: &Path) -> PathBuf {
    if let Ok(path) = fs::canonicalize(path) {
        return path;
    }

    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => canonical(parent).join(name),
        _ => path.to_owned(),
    }
}

impl Registry {
    /// the registry stored in `file`
    pub fn new(file: impl Into<PathBuf>) -> Self {
        Self { file: file.into() }
    }

    /// every known collection
    ///
    /// # Errors
    /// when the file cannot be read or is broken
    pub fn entries(&self) -> Result<Vec<Entry>, StoreError> {
        let Some(text) = read_optional(&self.file)? else {
            return Ok(Vec::new());
        };

        let file: RegistryFile =
            toml::from_str(&text).map_err(|e| StoreError::parse(&self.file, e.message()))?;

        file.entries
            .into_iter()
            .map(|e| {
                let id = LocalId::new(&e.id).ok_or_else(|| {
                    StoreError::parse(&self.file, format!("invalid id {:?}", e.id))
                })?;

                Ok(Entry {
                    path: e.path,
                    id,
                    trusted: e.trusted,
                    active_environment: e.active_environment.as_deref().and_then(EnvId::new),
                })
            })
            .collect()
    }

    /// the entry for `path`, if it is known
    ///
    /// # Errors
    /// when the registry cannot be read
    pub fn find(&self, path: &Path) -> Result<Option<Entry>, StoreError> {
        let path = canonical(path);

        Ok(self.entries()?.into_iter().find(|e| e.path == path))
    }

    /// reads, changes and writes the registry while holding its lock, so
    /// concurrent curst processes lose no entry
    fn update<T>(&self, change: impl FnOnce(&mut Vec<Entry>) -> T) -> Result<T, StoreError> {
        with_lock(&self.file, || {
            let mut entries = self.entries()?;
            let result = change(&mut entries);
            let file = RegistryFile {
                entries: entries
                    .into_iter()
                    .map(|e| EntryFile {
                        path: e.path,
                        id: e.id.as_str().to_owned(),
                        trusted: e.trusted,
                        active_environment: e.active_environment.map(|id| id.as_str().to_owned()),
                    })
                    .collect(),
            };

            write_atomic(&self.file, to_toml(&file).as_bytes())?;

            Ok(result)
        })
    }

    /// the entry for `path`, added with a new local id when it is not known;
    /// an existing entry keeps its id and trust
    ///
    /// # Errors
    /// when the registry cannot be read or written
    pub fn register(&self, path: &Path, trusted: bool) -> Result<Entry, StoreError> {
        let path = canonical(path);
        let id = ids::local_id()?;

        self.update(|entries| {
            if let Some(entry) = entries.iter().find(|e| e.path == path) {
                return entry.clone();
            }

            let entry = Entry {
                path,
                id,
                trusted,
                active_environment: None,
            };

            entries.push(entry.clone());

            entry
        })
    }

    /// follows a collection folder that curst renamed or moved, keeping its
    /// id and trust
    ///
    /// # Errors
    /// when the registry cannot be read or written
    pub fn moved(&self, from: &Path, to: &Path) -> Result<(), StoreError> {
        let from = canonical(from);
        let to = canonical(to);

        self.update(|entries| {
            for entry in entries.iter_mut().filter(|e| e.path == from) {
                entry.path.clone_from(&to);
            }
        })
    }

    /// drops the entry for `path`
    ///
    /// # Errors
    /// when the registry cannot be read or written
    pub fn forget(&self, path: &Path) -> Result<(), StoreError> {
        let path = canonical(path);

        self.update(|entries| entries.retain(|e| e.path != path))
    }

    /// remembers the active environment of the collection at `path`
    ///
    /// # Errors
    /// when the registry cannot be read or written
    pub fn set_active_environment(
        &self,
        path: &Path,
        id: Option<&EnvId>,
    ) -> Result<(), StoreError> {
        let path = canonical(path);

        self.update(|entries| {
            for entry in entries.iter_mut().filter(|e| e.path == path) {
                entry.active_environment = id.cloned();
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Registry, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let registry = Registry::new(dir.path().join("data/registry.toml"));
        let collection = dir.path().join("c");

        fs::create_dir(&collection).unwrap();

        (dir, registry, collection)
    }

    #[test]
    fn registering_is_idempotent() {
        let (_dir, registry, collection) = setup();

        assert_eq!(registry.entries().unwrap(), Vec::new());

        let first = registry.register(&collection, true).unwrap();
        let spelled = collection.join("..").join("c");
        let again = registry.register(&spelled, false).unwrap();

        assert_eq!(first, again);
        assert!(again.trusted);
        assert_eq!(first.path, fs::canonicalize(&collection).unwrap());
        assert_eq!(registry.entries().unwrap().len(), 1);
        assert_eq!(registry.find(&spelled).unwrap(), Some(first));
    }

    #[test]
    fn a_move_keeps_id_and_trust() {
        let (dir, registry, collection) = setup();
        let entry = registry.register(&collection, true).unwrap();
        let target = dir.path().join("renamed");

        fs::rename(&collection, &target).unwrap();
        registry.moved(&collection, &target).unwrap();

        let moved = registry.find(&target).unwrap().unwrap();

        assert_eq!((moved.id, moved.trusted), (entry.id, true));
        assert_eq!(registry.find(&collection).unwrap(), None);
    }

    #[test]
    fn forgetting_and_the_active_environment() {
        let (dir, registry, collection) = setup();
        let other = dir.path().join("other");
        let env = EnvId::new("e1").unwrap();

        fs::create_dir(&other).unwrap();
        registry.register(&collection, true).unwrap();
        registry.register(&other, false).unwrap();
        registry
            .set_active_environment(&collection, Some(&env))
            .unwrap();

        assert_eq!(
            registry
                .find(&collection)
                .unwrap()
                .unwrap()
                .active_environment,
            Some(env)
        );

        registry.set_active_environment(&collection, None).unwrap();

        assert_eq!(
            registry
                .find(&collection)
                .unwrap()
                .unwrap()
                .active_environment,
            None
        );

        registry.forget(&collection).unwrap();

        let left: Vec<_> = registry
            .entries()
            .unwrap()
            .into_iter()
            .map(|e| e.trusted)
            .collect();

        assert_eq!(left, [false]);
    }

    #[test]
    fn a_broken_registry_is_an_error() {
        let (_dir, registry, collection) = setup();

        fs::create_dir_all(registry.file.parent().unwrap()).unwrap();
        fs::write(&registry.file, "collections = 1").unwrap();

        assert!(matches!(registry.entries(), Err(StoreError::Parse { .. })));
        assert!(registry.register(&collection, true).is_err());

        fs::write(
            &registry.file,
            "[[collections]]\npath = \"/x\"\nid = \"BAD\"\n",
        )
        .unwrap();

        assert!(matches!(registry.entries(), Err(StoreError::Parse { .. })));
    }

    #[test]
    fn missing_paths_are_still_canonical() {
        let (_dir, _registry, collection) = setup();
        let real = fs::canonicalize(&collection).unwrap();

        assert_eq!(
            canonical(&collection.join("gone/deeper")),
            real.join("gone/deeper")
        );
        assert_eq!(canonical(Path::new("")), PathBuf::new());
    }
}
