//! checks that hold across modules: no secret value is ever written to a
//! file curst keeps, and a copied collection never reaches another one's
//! secrets

use std::fs;
use std::path::Path;
use std::sync::Arc;

use crate::model::{KeyValue, Request, Variable};
use crate::secrets::{EncryptedFile, MemoryStore, Secret, SecretIndex, SecretKey, Secrets};
use crate::store::{Backend, CollectionDir, CollectionStore, Library, Paths, Settings};
use crate::vars::VarName;

const PLANTED: [&str; 4] = [
    "PLANTED-env-7f3a",
    "PLANTED-collection-91c2",
    "PLANTED-global-55e0",
    "PLANTED-renamed-0b4d",
];
const HANDWRITTEN: &str = "HANDWRITTEN-a8e1";
const CANARY: &str = "CANARY-plain-3d9f";

fn name(text: &str) -> VarName {
    VarName::new(text).unwrap()
}

/// a secret variable that still holds its value in memory, as the editor
/// would just before saving
fn holding(text: &str, value: &str) -> Variable {
    Variable {
        value: value.into(),
        ..Variable::secret(name(text))
    }
}

/// every file under `root`, lock and temporary files included
fn every_file(root: &Path, found: &mut Vec<(String, Vec<u8>)>) {
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();

        if entry.file_type().unwrap().is_dir() {
            every_file(&path, found);
        } else {
            found.push((path.display().to_string(), fs::read(&path).unwrap()));
        }
    }
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[test]
fn no_secret_value_reaches_any_file() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let library = Library::new(paths.clone());
    let pets = library.create_collection("Pets").unwrap();
    let id = library.registry().find(pets.root()).unwrap().unwrap().id;
    let mut dev = pets.create_environment("Dev").unwrap();

    // variables saved while secret values sit in memory
    dev.variables = vec![
        holding("token", PLANTED[0]),
        Variable::new(name("plain"), CANARY),
    ];
    pets.save_environment(&dev).unwrap();
    pets.save_settings("Pets", &[holding("key", PLANTED[1])])
        .unwrap();
    library
        .save_globals(&[holding("global", PLANTED[2])])
        .unwrap();

    let request = Request {
        url: "https://api.example.com/{{key}}".into(),
        headers: vec![KeyValue::new("Authorization", "Bearer {{token}}")],
        ..Request::default()
    };

    pets.create_request(&[], "Get", &request).unwrap();
    Settings {
        secrets_backend: Some(Backend::EncryptedFile),
    }
    .save(&paths.config_file())
    .unwrap();

    // the store: set, approve, rename and migrate twice
    let file = || {
        Arc::new(EncryptedFile::new(paths.secrets_file(), Secret::new("pass")).with_work_factor(2))
    };
    let secrets = Secrets::new(SecretIndex::new(paths.secrets_index_file()), file());
    let env_key = SecretKey::Environment {
        collection: id.clone(),
        environment: dev.id.clone(),
        name: name("token"),
    };
    let collection_key = SecretKey::Collection {
        collection: id.clone(),
        name: name("key"),
    };
    let old_key = SecretKey::Global { name: name("old") };

    secrets.set(&env_key, &Secret::new(PLANTED[0])).unwrap();
    secrets
        .set(&collection_key, &Secret::new(PLANTED[1]))
        .unwrap();
    secrets
        .set(
            &SecretKey::Global {
                name: name("global"),
            },
            &Secret::new(PLANTED[2]),
        )
        .unwrap();
    secrets.set(&old_key, &Secret::new(PLANTED[3])).unwrap();
    secrets
        .index()
        .approve(std::slice::from_ref(&env_key), "https://api.example.com")
        .unwrap();
    secrets
        .rename(&old_key, &SecretKey::Global { name: name("new") })
        .unwrap();

    let secrets = secrets.migrate(Arc::new(MemoryStore::new())).unwrap();
    let secrets = secrets.migrate(file()).unwrap();

    assert_eq!(
        secrets.get(&env_key).unwrap(),
        Some(Secret::new(PLANTED[0]))
    );

    library.rename_collection(&pets, "Animals").unwrap();

    // the scan, over everything under the temporary root
    let mut files = Vec::new();

    every_file(dir.path(), &mut files);

    assert!(
        files.iter().any(|(_, bytes)| contains(bytes, CANARY)),
        "the canary was not found, so the scan proves nothing"
    );

    for (path, bytes) in &files {
        for value in PLANTED {
            assert!(!contains(bytes, value), "{value} was written to {path}");
        }
    }
}

#[test]
fn a_secret_value_written_by_hand_is_gone_after_a_save() {
    let dir = tempfile::tempdir().unwrap();
    let pets = CollectionDir::create(dir.path().join("pets"), "Pets").unwrap();
    let mut dev = pets.create_environment("Dev").unwrap();

    dev.variables = vec![Variable::secret(name("token"))];
    pets.save_environment(&dev).unwrap();

    let env_file = pets.root().join("environments/dev.toml");
    let text = fs::read_to_string(&env_file).unwrap();

    fs::write(
        &env_file,
        text.replace(
            "secret = true",
            &format!("secret = true\nvalue = \"{HANDWRITTEN}\""),
        ),
    )
    .unwrap();

    let loaded = pets.load().unwrap();

    assert!(
        loaded
            .issues
            .iter()
            .any(|i| i.message.starts_with("token is a secret"))
    );

    pets.save_environment(&loaded.collection.environments[0])
        .unwrap();

    let mut files = Vec::new();

    every_file(dir.path(), &mut files);

    for (path, bytes) in &files {
        assert!(!contains(bytes, HANDWRITTEN), "still in {path}");
    }
}

#[test]
fn a_copied_collection_reaches_none_of_the_original_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path().join("home"));
    let library = Library::new(paths.clone());
    let original = library.create_collection("Pets").unwrap();
    let original_id = library
        .registry()
        .find(original.root())
        .unwrap()
        .unwrap()
        .id;
    let mut dev = original.create_environment("Dev").unwrap();

    dev.variables = vec![Variable::secret(name("token"))];
    original.save_environment(&dev).unwrap();

    let secrets = Secrets::new(
        SecretIndex::new(paths.secrets_index_file()),
        Arc::new(MemoryStore::new()),
    );

    secrets
        .set(
            &SecretKey::Environment {
                collection: original_id.clone(),
                environment: dev.id.clone(),
                name: name("token"),
            },
            &Secret::new("the original's"),
        )
        .unwrap();

    // a repository ships a copy: same environment id, same variable name
    let project = dir.path().join("repo");
    let copy = project.join(".curst");

    fs::create_dir_all(copy.join("environments")).unwrap();
    fs::copy(
        original.root().join("collection.toml"),
        copy.join("collection.toml"),
    )
    .unwrap();
    fs::copy(
        original.root().join("environments/dev.toml"),
        copy.join("environments/dev.toml"),
    )
    .unwrap();

    let local = Library::local_collection(&project).unwrap();
    let entry = library.registry().register(local.root(), false).unwrap();
    let loaded = local.load().unwrap().collection;

    assert!(!entry.trusted);
    assert_ne!(entry.id, original_id);
    assert_eq!(loaded.environments[0].id, dev.id);

    let key = SecretKey::Environment {
        collection: entry.id,
        environment: loaded.environments[0].id.clone(),
        name: name("token"),
    };

    assert_eq!(secrets.get(&key).unwrap(), None);
    assert!(!library.is_managed(&CollectionDir::open(copy)));
}
