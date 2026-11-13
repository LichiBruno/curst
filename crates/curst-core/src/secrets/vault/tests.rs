use std::sync::Mutex;

use super::*;
use crate::model::EnvId;
use crate::secrets::MemoryStore;
use crate::vars::VarName;

/// a store that fails on chosen calls, counted across get, set and delete
#[derive(Default)]
struct Flaky {
    inner: MemoryStore,
    calls: Mutex<usize>,
    fail_on: Mutex<Vec<usize>>,
}

impl Flaky {
    fn fail_on(&self, calls: &[usize]) {
        *self.calls.lock().unwrap() = 0;
        *self.fail_on.lock().unwrap() = calls.to_vec();
    }

    fn tick(&self) -> Result<(), SecretError> {
        let mut calls = self.calls.lock().unwrap();

        *calls += 1;

        if self.fail_on.lock().unwrap().contains(&calls) {
            Err(SecretError::Backend("injected".into()))
        } else {
            Ok(())
        }
    }
}

impl SecretStore for Flaky {
    fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretError> {
        self.tick()?;

        self.inner.get(key)
    }

    fn set(&self, key: &SecretKey, value: &Secret) -> Result<(), SecretError> {
        self.tick()?;

        self.inner.set(key, value)
    }

    fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        self.tick()?;

        self.inner.delete(key)
    }
}

fn lid(text: &str) -> LocalId {
    LocalId::new(text).unwrap()
}

fn env_key(collection: &str, environment: &str, name: &str) -> SecretKey {
    SecretKey::Environment {
        collection: lid(collection),
        environment: EnvId::new(environment).unwrap(),
        name: VarName::new(name).unwrap(),
    }
}

fn global(name: &str) -> SecretKey {
    SecretKey::Global {
        name: VarName::new(name).unwrap(),
    }
}

fn setup() -> (tempfile::TempDir, Arc<Flaky>, Secrets) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Flaky::default());
    let secrets = Secrets::new(
        SecretIndex::new(dir.path().join("secrets-index.toml")),
        store.clone(),
    );

    (dir, store, secrets)
}

fn indexed(secrets: &Secrets) -> Vec<String> {
    secrets
        .list()
        .unwrap()
        .into_iter()
        .map(|e| e.key.to_string())
        .collect()
}

/// the rule every test checks: no stored value without an index entry
fn no_orphans(store: &Flaky, secrets: &Secrets) {
    let listed = indexed(secrets);

    for key in store.inner.keys() {
        assert!(listed.contains(&key.to_string()), "{key} has no entry");
    }
}

#[test]
fn set_get_and_delete_keep_the_index() {
    let (_dir, store, secrets) = setup();
    let key = global("token");

    secrets.set(&key, &Secret::new("v1")).unwrap();
    secrets.set(&key, &Secret::new("v2")).unwrap();

    assert_eq!(secrets.get(&key).unwrap(), Some(Secret::new("v2")));
    assert_eq!(indexed(&secrets), ["curst/global/token"]);

    secrets.delete(&key).unwrap();

    assert_eq!(indexed(&secrets), Vec::<String>::new());
    assert_eq!(store.inner.keys(), Vec::new());
    assert!(format!("{secrets:?}").contains("Secrets"));
    assert!(Arc::ptr_eq(
        secrets.store(),
        &(store.clone() as Arc<dyn SecretStore>)
    ));
}

#[test]
fn a_failed_write_leaves_no_orphan_and_no_stray_entry() {
    let (_dir, store, secrets) = setup();
    let key = global("token");

    store.fail_on(&[1]);

    assert!(secrets.set(&key, &Secret::new("v")).is_err());
    assert_eq!(indexed(&secrets), Vec::<String>::new());

    // an existing entry survives a failed overwrite
    secrets.set(&key, &Secret::new("v")).unwrap();
    store.fail_on(&[1]);

    assert!(secrets.set(&key, &Secret::new("w")).is_err());
    assert_eq!(indexed(&secrets), ["curst/global/token"]);
    assert_eq!(secrets.get(&key).unwrap(), Some(Secret::new("v")));
    no_orphans(&store, &secrets);
}

#[test]
fn a_failed_delete_keeps_the_entry() {
    let (_dir, store, secrets) = setup();
    let key = global("token");

    secrets.set(&key, &Secret::new("v")).unwrap();
    store.fail_on(&[1]);

    assert!(secrets.delete(&key).is_err());
    assert_eq!(indexed(&secrets), ["curst/global/token"]);
    no_orphans(&store, &secrets);
}

#[test]
fn scopes_are_counted_and_deleted_together() {
    let (_dir, store, secrets) = setup();

    for key in [
        env_key("abc", "e1", "a"),
        env_key("abc", "e2", "b"),
        env_key("abcd", "e1", "c"),
        SecretKey::Collection {
            collection: lid("abc"),
            name: VarName::new("d").unwrap(),
        },
        global("g"),
    ] {
        secrets.set(&key, &Secret::new("v")).unwrap();
    }

    let env = SecretScope::Environment(lid("abc"), EnvId::new("e1").unwrap());
    let collection = SecretScope::Collection(lid("abc"));

    assert_eq!(secrets.count(&env).unwrap(), 1);
    assert_eq!(secrets.count(&collection).unwrap(), 3);
    assert_eq!(secrets.delete_scope(&env).unwrap(), 1);
    assert_eq!(secrets.delete_scope(&collection).unwrap(), 2);
    assert_eq!(indexed(&secrets), ["curst/abcd/env/e1/c", "curst/global/g"]);
    no_orphans(&store, &secrets);
}

#[test]
fn a_scope_delete_that_fails_halfway_keeps_the_rest_listed() {
    let (_dir, store, secrets) = setup();

    for name in ["a", "b", "c"] {
        secrets
            .set(&env_key("abc", "e1", name), &Secret::new("v"))
            .unwrap();
    }

    store.fail_on(&[2]);

    assert!(
        secrets
            .delete_scope(&SecretScope::Collection(lid("abc")))
            .is_err()
    );
    assert_eq!(indexed(&secrets).len(), 2);
    no_orphans(&store, &secrets);
}

#[test]
fn a_rename_moves_the_value_and_the_hosts() {
    let (_dir, store, secrets) = setup();
    let from = env_key("abc", "e1", "old");
    let to = env_key("abc", "e1", "new");

    secrets.set(&from, &Secret::new("v")).unwrap();
    secrets
        .index
        .locked(|locked| {
            locked.entries[0].hosts = vec!["https://h:443".into()];
            locked.save()
        })
        .unwrap();
    secrets.rename(&from, &to).unwrap();

    assert_eq!(secrets.get(&to).unwrap(), Some(Secret::new("v")));
    assert_eq!(secrets.get(&from).unwrap(), None);
    assert_eq!(
        secrets.list().unwrap(),
        [IndexEntry {
            key: to.clone(),
            hosts: vec!["https://h:443".into()],
        }]
    );
    assert!(matches!(
        secrets.rename(&from, &to),
        Err(SecretError::NotFound(_))
    ));

    secrets.rename(&to, &to).unwrap();

    assert_eq!(secrets.get(&to).unwrap(), Some(Secret::new("v")));

    // the copy fails: the old secret stays, listed
    store.fail_on(&[2]);

    assert!(secrets.rename(&to, &from).is_err());
    assert_eq!(indexed(&secrets), [to.to_string()]);
    no_orphans(&store, &secrets);
}

#[test]
fn migration_copies_checks_then_deletes() {
    let (_dir, store, secrets) = setup();

    for name in ["a", "b"] {
        secrets.set(&global(name), &Secret::new(name)).unwrap();
    }

    // an entry whose value was lost is skipped
    store.inner.delete(&global("b")).unwrap();

    let target = Arc::new(Flaky::default());

    target.fail_on(&[1]);

    assert!(secrets.migrate(target.clone()).is_err());
    assert_eq!(store.inner.keys(), [global("a")]);

    let moved = secrets.migrate(target.clone()).unwrap();

    assert_eq!(store.inner.keys(), Vec::new());
    assert_eq!(target.inner.keys(), [global("a")]);
    assert_eq!(moved.get(&global("a")).unwrap(), Some(Secret::new("a")));
    assert_eq!(indexed(&moved), ["curst/global/a", "curst/global/b"]);
}

#[test]
fn a_copy_that_reads_back_wrong_stops_the_migration() {
    struct Liar;

    impl SecretStore for Liar {
        fn get(&self, _: &SecretKey) -> Result<Option<Secret>, SecretError> {
            Ok(Some(Secret::new("other")))
        }

        fn set(&self, _: &SecretKey, _: &Secret) -> Result<(), SecretError> {
            Ok(())
        }

        fn delete(&self, _: &SecretKey) -> Result<(), SecretError> {
            Ok(())
        }
    }

    let (_dir, store, secrets) = setup();

    secrets.set(&global("a"), &Secret::new("a")).unwrap();

    assert!(matches!(
        secrets.migrate(Arc::new(Liar)),
        Err(SecretError::Backend(_))
    ));
    assert_eq!(store.inner.keys(), [global("a")]);
    assert!(Liar.probe().is_err());
}

#[test]
fn pruning_forgets_unknown_collections_only() {
    let (_dir, _store, secrets) = setup();

    for key in [
        env_key("gone", "e1", "a"),
        env_key("kept", "e1", "b"),
        global("g"),
    ] {
        secrets.set(&key, &Secret::new("v")).unwrap();
    }

    assert_eq!(secrets.prune(&[lid("kept")]).unwrap(), 1);
    assert_eq!(indexed(&secrets), ["curst/kept/env/e1/b", "curst/global/g"]);
}
