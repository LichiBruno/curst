//! every collection in the data folder, and the global variables

use std::fs;

use super::collection::CollectionDir;
use super::format::{GlobalsFile, to_toml, variables_from_file, variables_to_file};
use super::fs::{read_optional, write_atomic};
use super::{Paths, Registry, StoreError, slug};
use crate::model::Variable;

/// the collections in `<data dir>/collections` and the files beside them
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    paths: Paths,
}

impl Library {
    /// the library stored under `paths`
    #[must_use]
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }

    /// the directories in use
    #[must_use]
    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// the registry of known collections
    #[must_use]
    pub fn registry(&self) -> Registry {
        Registry::new(self.paths.registry_file())
    }

    /// every collection in the data folder, sorted by folder name; each one
    /// is registered, so it has a local id
    ///
    /// # Errors
    /// when the folder or the registry cannot be read
    pub fn collections(&self) -> Result<Vec<CollectionDir>, StoreError> {
        let dir = self.paths.collections_dir();

        if !dir.is_dir() {
            return Ok(Vec::new());
        }

        let mut found = Vec::new();

        for entry in fs::read_dir(&dir).map_err(|e| StoreError::io(&dir, e))? {
            let path = entry.map_err(|e| StoreError::io(&dir, e))?.path();
            let collection = CollectionDir::open(&path);
            let hidden = path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'));

            if !hidden && collection.exists() {
                found.push(collection);
            }
        }

        found.sort_by(|a, b| a.root().cmp(b.root()));

        let registry = self.registry();

        for collection in &found {
            registry.register(collection.root(), true)?;
        }

        Ok(found)
    }

    /// creates a collection in the data folder, trusted, since this user
    /// made it here
    ///
    /// # Errors
    /// when its folder or the registry cannot be written
    pub fn create_collection(&self, name: &str) -> Result<CollectionDir, StoreError> {
        let dir = self.paths.collections_dir();
        let slug = slug::unique(&slug::slugify(name), |s| dir.join(s).exists());
        let collection = CollectionDir::create(dir.join(slug), name)?;

        self.registry().register(collection.root(), true)?;

        Ok(collection)
    }

    /// renames a collection's folder after its new name, keeping its
    /// registry entry; the display name is changed through the store
    ///
    /// # Errors
    /// when the folder cannot be renamed or the registry written
    pub fn rename_collection(
        &self,
        collection: &CollectionDir,
        name: &str,
    ) -> Result<CollectionDir, StoreError> {
        let dir = self.paths.collections_dir();
        let current = collection
            .root()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        let slug = slug::unique(&slug::slugify(name), |s| {
            Some(s) != current.as_deref() && dir.join(s).exists()
        });
        let target = dir.join(slug);

        if target != collection.root() {
            fs::rename(collection.root(), &target).map_err(|e| StoreError::io(&target, e))?;
            self.registry().moved(collection.root(), &target)?;
        }

        Ok(CollectionDir::open(target))
    }

    /// deletes a collection folder and forgets it
    ///
    /// # Errors
    /// when the folder cannot be removed or the registry written
    pub fn delete_collection(&self, collection: &CollectionDir) -> Result<(), StoreError> {
        self.registry().forget(collection.root())?;

        fs::remove_dir_all(collection.root()).map_err(|e| StoreError::io(collection.root(), e))
    }

    /// the variables every collection can use
    ///
    /// # Errors
    /// when `globals.toml` cannot be read or is broken
    pub fn globals(&self) -> Result<Vec<Variable>, StoreError> {
        let path = self.paths.globals_file();
        let Some(text) = read_optional(&path)? else {
            return Ok(Vec::new());
        };

        let file: GlobalsFile =
            toml::from_str(&text).map_err(|e| StoreError::parse(&path, e.message()))?;

        variables_from_file(file.variables).map_err(|e| StoreError::parse(&path, e))
    }

    /// saves the global variables
    ///
    /// # Errors
    /// when `globals.toml` cannot be written
    pub fn save_globals(&self, variables: &[Variable]) -> Result<(), StoreError> {
        let file = GlobalsFile {
            variables: variables_to_file(variables),
        };

        write_atomic(&self.paths.globals_file(), to_toml(&file).as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::CollectionStore;
    use crate::vars::VarName;

    fn library() -> (tempfile::TempDir, Library) {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::new(Paths::under(dir.path()));

        (dir, library)
    }

    #[test]
    fn collections_are_created_listed_and_registered() {
        let (_dir, library) = library();

        assert_eq!(library.collections().unwrap(), Vec::new());

        let pets = library.create_collection("Pets API").unwrap();
        let again = library.create_collection("Pets api").unwrap();

        fs::create_dir_all(library.paths().collections_dir().join("not-a-collection")).unwrap();
        fs::create_dir_all(library.paths().collections_dir().join(".hidden")).unwrap();

        let found = library.collections().unwrap();

        assert_eq!(found, [pets.clone(), again.clone()]);
        assert!(pets.root().ends_with("pets-api"));
        assert!(again.root().ends_with("pets-api-2"));
        assert_eq!(library.registry().entries().unwrap().len(), 2);
        assert!(
            library
                .registry()
                .find(pets.root())
                .unwrap()
                .unwrap()
                .trusted
        );
    }

    #[test]
    fn hand_made_collections_get_registered() {
        let (_dir, library) = library();
        let root = library.paths().collections_dir().join("manual");

        CollectionDir::create(&root, "Manual").unwrap();

        assert_eq!(library.registry().find(&root).unwrap(), None);
        assert_eq!(library.collections().unwrap().len(), 1);
        assert!(library.registry().find(&root).unwrap().is_some());
    }

    #[test]
    fn renaming_and_deleting_keep_the_registry_in_step() {
        let (_dir, library) = library();
        let pets = library.create_collection("Pets").unwrap();
        let id = library.registry().find(pets.root()).unwrap().unwrap().id;
        let renamed = library.rename_collection(&pets, "Animals").unwrap();

        assert!(renamed.root().ends_with("animals"));
        assert_eq!(
            library.registry().find(renamed.root()).unwrap().unwrap().id,
            id
        );
        assert_eq!(renamed.load().unwrap().collection.name, "Pets");
        assert_eq!(
            library.rename_collection(&renamed, "ANIMALS").unwrap(),
            renamed
        );

        library.delete_collection(&renamed).unwrap();

        assert!(!renamed.root().exists());
        assert_eq!(library.registry().entries().unwrap(), Vec::new());
    }

    #[test]
    fn globals_round_trip() {
        let (_dir, library) = library();
        let globals = vec![Variable::new(VarName::new("user").unwrap(), "me")];

        assert_eq!(library.globals().unwrap(), Vec::new());

        library.save_globals(&globals).unwrap();

        assert_eq!(library.globals().unwrap(), globals);

        fs::write(library.paths().globals_file(), "variables = 1").unwrap();

        assert!(library.globals().is_err());

        fs::write(
            library.paths().globals_file(),
            "[[variables]]\nname = \"a b\"",
        )
        .unwrap();

        assert!(library.globals().is_err());
    }
}
