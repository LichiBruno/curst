//! the secret store of this session: opened the first time a secret is
//! needed, never at startup, and run on its own thread, since opening the
//! encrypted file takes a moment and a keychain may show a dialog

use std::path::PathBuf;
use std::sync::Arc;

use curst_core::secrets::{
    EncryptedFile, Keychain, Secret, SecretError, SecretIndex, SecretKey, SecretStore, Secrets,
};
use curst_core::store::{Backend, Paths, Settings};

use crate::app::{Action, Locked, SecretChange};

type OpenKeychain = Box<dyn Fn() -> Result<Arc<dyn SecretStore>, SecretError> + Send>;
type OpenFile = Box<dyn Fn(Secret) -> Arc<dyn SecretStore> + Send>;

/// work for the secrets thread, done in the order it arrives
#[derive(Debug)]
pub(crate) enum Job {
    /// the values of `keys`, for the send waiting under `id`
    Load { id: u64, keys: Vec<SecretKey> },

    /// opens the encrypted file with a passphrase
    Unlock(Secret),

    /// records the backend the user picked
    Choose(Backend),

    /// applies `changes` in order, for the save waiting under `id`
    Change { id: u64, changes: Vec<SecretChange> },

    /// moves every secret to the other backend; a passphrase is needed when
    /// that is the encrypted file
    Migrate {
        id: u64,
        to: Backend,
        passphrase: Option<Secret>,
    },

    /// lets `keys` go to `origin`, for the send waiting under `id`
    Approve {
        id: u64,
        keys: Vec<SecretKey>,
        origin: String,
    },
}

pub(crate) struct Session {
    config_file: PathBuf,
    secrets_file: PathBuf,
    index: SecretIndex,
    backend: Option<Backend>,
    secrets: Option<Secrets>,
    keychain: OpenKeychain,
    file: OpenFile,
}

impl Session {
    /// the session for these paths, with the real keychain and file
    pub(crate) fn new(paths: &Paths) -> Self {
        let path = paths.secrets_file();

        Self::with(
            paths,
            Box::new(|| {
                let keychain = Keychain::open()?;

                keychain.probe()?;

                Ok(Arc::new(keychain) as Arc<dyn SecretStore>)
            }),
            Box::new(move |passphrase| Arc::new(EncryptedFile::new(&path, passphrase))),
        )
    }

    /// a session that opens its stores with the given functions
    pub(crate) fn with(paths: &Paths, keychain: OpenKeychain, file: OpenFile) -> Self {
        let config_file = paths.config_file();
        let backend = Settings::load(&config_file)
            .ok()
            .and_then(|(settings, _)| settings.secrets_backend);

        Self {
            config_file,
            secrets_file: paths.secrets_file(),
            index: SecretIndex::new(paths.secrets_index_file()),
            backend,
            secrets: None,
            keychain,
            file,
        }
    }

    /// the open store, or what is needed to open it
    fn open(&mut self) -> Result<&Secrets, Locked> {
        if self.secrets.is_none() {
            let store = match self.backend {
                None => return Err(Locked::Backend),
                Some(Backend::EncryptedFile) => {
                    return Err(Locked::Passphrase {
                        new: !self.secrets_file.exists(),
                        error: None,
                    });
                }
                Some(Backend::Keychain) => {
                    (self.keychain)().map_err(|e| Locked::Failed(e.to_string()))?
                }
            };

            self.secrets = Some(Secrets::new(self.index.clone(), store));
        }

        self.secrets.as_ref().ok_or(Locked::Backend)
    }

    pub(crate) fn run(&mut self, job: Job) -> Action {
        match job {
            Job::Load { id, keys } => match self.open() {
                Err(locked) => Action::SecretsLocked {
                    id: Some(id),
                    locked,
                },
                Ok(secrets) => {
                    let values: Result<Vec<_>, SecretError> = keys
                        .into_iter()
                        .map(|key| secrets.get(&key).map(|value| (key, value)))
                        .collect();

                    match values {
                        Ok(values) => Action::SecretsLoaded { id, values },
                        Err(err) => Action::SecretsLocked {
                            id: Some(id),
                            locked: Locked::Failed(err.to_string()),
                        },
                    }
                }
            },
            Job::Unlock(passphrase) => {
                let store = (self.file)(passphrase);

                // reading any key opens the file and checks the passphrase
                match store.get(&SecretKey::Probe) {
                    Ok(_) => {
                        self.secrets = Some(Secrets::new(self.index.clone(), store));

                        Action::Unlocked
                    }
                    Err(SecretError::WrongPassphrase) => Action::SecretsLocked {
                        id: None,
                        locked: Locked::Passphrase {
                            new: false,
                            error: Some("wrong passphrase, try again".into()),
                        },
                    },
                    Err(err) => Action::SecretsLocked {
                        id: None,
                        locked: Locked::Failed(err.to_string()),
                    },
                }
            }
            Job::Choose(backend) => Action::BackendChosen(self.choose(backend)),
            Job::Change { id, changes } => match self.open() {
                Err(locked) => Action::SecretsLocked {
                    id: Some(id),
                    locked,
                },
                Ok(secrets) => Action::SecretsChanged {
                    id,
                    result: apply(secrets, changes).map_err(|e| e.to_string()),
                },
            },
            Job::Migrate { id, to, passphrase } => self.migrate(id, to, passphrase),
            Job::Approve { id, keys, origin } => Action::Approved {
                id,
                result: self
                    .index
                    .approve(&keys, &origin)
                    .map_err(|e| e.to_string()),
            },
        }
    }

    /// copies every secret to the other backend and checks each copy, then
    /// records the switch, and only then deletes the old copies
    fn migrate(&mut self, id: u64, to: Backend, passphrase: Option<Secret>) -> Action {
        let done = |result| Action::Migrated { id, result };

        if self.backend == Some(to) {
            return done(Err("secrets are already kept there".into()));
        }

        let source = match self.open() {
            Ok(source) => source.clone(),
            Err(locked) => {
                return Action::SecretsLocked {
                    id: Some(id),
                    locked,
                };
            }
        };
        let target = match (to, passphrase) {
            (Backend::Keychain, _) => match (self.keychain)() {
                Ok(store) => store,
                Err(err) => return done(Err(format!("the keychain cannot be used here: {err}"))),
            },
            (Backend::EncryptedFile, Some(passphrase)) => (self.file)(passphrase),
            (Backend::EncryptedFile, None) => {
                return done(Err("the encrypted file needs a passphrase".into()));
            }
        };

        if let Err(err) = target.get(&SecretKey::Probe) {
            return done(Err(match err {
                SecretError::WrongPassphrase => {
                    "secrets.age already exists with another passphrase".into()
                }
                other => other.to_string(),
            }));
        }

        let (moved, keys) = match source.copy_to(target) {
            Ok(copied) => copied,
            Err(err) => return done(Err(format!("nothing was moved: {err}"))),
        };
        if let Err(err) = Settings::set_backend(&self.config_file, Some(to)) {
            return done(Err(format!("nothing was moved: {err}")));
        }

        self.backend = Some(to);
        self.secrets = Some(moved);

        match source.forget_values(&keys) {
            Ok(()) => done(Ok((to, keys.len()))),
            Err(err) => done(Err(format!(
                "secrets moved, but some old copies could not be deleted: {err}"
            ))),
        }
    }

    fn choose(&mut self, backend: Backend) -> Result<Backend, String> {
        if backend == Backend::Keychain {
            let store = (self.keychain)().map_err(|e| {
                format!(
                    "the keychain cannot be used here: {e}; the encrypted file works everywhere"
                )
            })?;

            self.secrets = Some(Secrets::new(self.index.clone(), store));
        } else {
            self.secrets = None;
        }

        Settings::set_backend(&self.config_file, Some(backend)).map_err(|e| e.to_string())?;
        self.backend = Some(backend);

        Ok(backend)
    }
}

fn apply(secrets: &Secrets, changes: Vec<SecretChange>) -> Result<(), SecretError> {
    for change in changes {
        match change {
            SecretChange::Set(key, value) => secrets.set(&key, &value)?,

            // a renamed secret that never had a value has nothing to move
            SecretChange::Rename(from, to) => match secrets.rename(&from, &to) {
                Ok(()) | Err(SecretError::NotFound(_)) => {}
                Err(err) => return Err(err),
            },
            SecretChange::Delete(key) => secrets.delete(&key)?,
            SecretChange::DeleteScope(scope) => {
                secrets.delete_scope(&scope)?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use curst_core::secrets::MemoryStore;
    use curst_core::vars::VarName;

    use super::*;

    fn token() -> SecretKey {
        SecretKey::Global {
            name: VarName::new("token").unwrap(),
        }
    }

    /// a session whose file only opens with "right", holding one token, and
    /// whose keychain works when `keychain_works`
    fn session(dir: &std::path::Path, keychain_works: bool) -> Session {
        let paths = Paths::under(dir);
        let index = SecretIndex::new(paths.secrets_index_file());
        let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());

        Secrets::new(index, store.clone())
            .set(&token(), &Secret::new("t0k"))
            .unwrap();

        let keychain_store = store.clone();

        Session::with(
            &paths,
            Box::new(move || {
                if keychain_works {
                    Ok(keychain_store.clone())
                } else {
                    Err(SecretError::Unavailable("no secret service".into()))
                }
            }),
            Box::new(move |passphrase| {
                if passphrase.expose() == "right" {
                    store.clone()
                } else {
                    Arc::new(Shut)
                }
            }),
        )
    }

    /// a store that never opens, like a file with another passphrase
    struct Shut;

    impl SecretStore for Shut {
        fn get(&self, _: &SecretKey) -> Result<Option<Secret>, SecretError> {
            Err(SecretError::WrongPassphrase)
        }

        fn set(&self, _: &SecretKey, _: &Secret) -> Result<(), SecretError> {
            Err(SecretError::WrongPassphrase)
        }

        fn delete(&self, _: &SecretKey) -> Result<(), SecretError> {
            Err(SecretError::WrongPassphrase)
        }
    }

    fn load(session: &mut Session) -> Action {
        session.run(Job::Load {
            id: 1,
            keys: vec![token()],
        })
    }

    #[test]
    fn the_first_load_asks_for_a_backend_then_a_passphrase() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = session(dir.path(), true);

        assert!(matches!(
            load(&mut session),
            Action::SecretsLocked {
                id: Some(1),
                locked: crate::app::Locked::Backend
            }
        ));
        assert!(matches!(
            session.run(Job::Choose(Backend::EncryptedFile)),
            Action::BackendChosen(Ok(Backend::EncryptedFile))
        ));
        assert_eq!(
            Settings::load(&Paths::under(dir.path()).config_file())
                .unwrap()
                .0
                .secrets_backend,
            Some(Backend::EncryptedFile)
        );
        assert!(matches!(
            load(&mut session),
            Action::SecretsLocked {
                locked: crate::app::Locked::Passphrase { new: true, .. },
                ..
            }
        ));
        assert!(matches!(
            session.run(Job::Unlock(Secret::new("wrong"))),
            Action::SecretsLocked {
                locked: crate::app::Locked::Passphrase { error: Some(_), .. },
                ..
            }
        ));
        assert!(matches!(
            session.run(Job::Unlock(Secret::new("right"))),
            Action::Unlocked
        ));

        let Action::SecretsLoaded { id, values } = load(&mut session) else {
            panic!("not loaded");
        };

        assert_eq!((id, values), (1, vec![(token(), Some(Secret::new("t0k")))]));
    }

    #[test]
    fn the_keychain_is_checked_when_chosen() {
        let dir = tempfile::tempdir().unwrap();
        let mut broken = session(dir.path(), false);

        let Action::BackendChosen(Err(message)) = broken.run(Job::Choose(Backend::Keychain)) else {
            panic!("chosen");
        };

        assert!(message.contains("encrypted file works everywhere"));
        assert!(
            Settings::load(&Paths::under(dir.path()).config_file())
                .unwrap()
                .0
                .secrets_backend
                .is_none()
        );

        let mut working = session(dir.path(), true);

        assert!(matches!(
            working.run(Job::Choose(Backend::Keychain)),
            Action::BackendChosen(Ok(Backend::Keychain))
        ));
        assert!(matches!(load(&mut working), Action::SecretsLoaded { .. }));

        // a later session opens the keychain itself, and reports when it can't
        let mut later = session(dir.path(), false);

        assert!(matches!(
            load(&mut later),
            Action::SecretsLocked {
                locked: crate::app::Locked::Failed(_),
                ..
            }
        ));
    }

    #[test]
    fn changes_are_applied_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = session(dir.path(), true);
        let key = |name: &str| SecretKey::Global {
            name: VarName::new(name).unwrap(),
        };

        session.run(Job::Choose(Backend::Keychain));

        let action = session.run(Job::Change {
            id: 5,
            changes: vec![
                SecretChange::Set(key("a"), Secret::new("1")),
                SecretChange::Rename(key("a"), key("b")),
                // a secret that never had a value has nothing to move
                SecretChange::Rename(key("never"), key("other")),
                SecretChange::Delete(key("token")),
            ],
        });

        assert!(matches!(
            action,
            Action::SecretsChanged {
                id: 5,
                result: Ok(())
            }
        ));

        let Action::SecretsLoaded { values, .. } = session.run(Job::Load {
            id: 6,
            keys: vec![key("a"), key("b"), token()],
        }) else {
            panic!("not loaded");
        };

        assert_eq!(
            values,
            [
                (key("a"), None),
                (key("b"), Some(Secret::new("1"))),
                (token(), None)
            ]
        );

        // a locked store asks before changing anything
        let mut locked = Session::with(
            &Paths::under(dir.path().join("other")),
            Box::new(|| Err(SecretError::Unavailable("none".into()))),
            Box::new(|_| Arc::new(Shut)),
        );

        assert!(matches!(
            locked.run(Job::Change {
                id: 7,
                changes: vec![SecretChange::Delete(key("a"))],
            }),
            Action::SecretsLocked {
                id: Some(7),
                locked: crate::app::Locked::Backend
            }
        ));
    }

    #[test]
    fn secrets_move_between_backends_and_the_switch_is_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let keychain: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
        let file: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
        let paths = Paths::under(dir.path());
        let index = SecretIndex::new(paths.secrets_index_file());

        Secrets::new(index, keychain.clone())
            .set(&token(), &Secret::new("t0k"))
            .unwrap();

        let (from_keychain, to_file) = (keychain.clone(), file.clone());
        let mut session = Session::with(
            &paths,
            Box::new(move || Ok(from_keychain.clone())),
            Box::new(move |passphrase| {
                if passphrase.expose() == "right" {
                    to_file.clone()
                } else {
                    Arc::new(Shut)
                }
            }),
        );

        session.run(Job::Choose(Backend::Keychain));

        let migrate = |session: &mut Session, to, passphrase: Option<&str>| {
            session.run(Job::Migrate {
                id: 9,
                to,
                passphrase: passphrase.map(Secret::new),
            })
        };
        let error = |action: Action| match action {
            Action::Migrated { result: Err(e), .. } => e,
            other => panic!("unexpected {other:?}"),
        };

        assert!(error(migrate(&mut session, Backend::Keychain, None)).contains("already"));
        assert!(error(migrate(&mut session, Backend::EncryptedFile, None)).contains("passphrase"));
        assert!(
            error(migrate(&mut session, Backend::EncryptedFile, Some("wrong")))
                .contains("another passphrase")
        );
        assert!(matches!(
            migrate(&mut session, Backend::EncryptedFile, Some("right")),
            Action::Migrated {
                id: 9,
                result: Ok((Backend::EncryptedFile, 1))
            }
        ));
        assert_eq!(keychain.get(&token()).unwrap(), None);
        assert_eq!(file.get(&token()).unwrap(), Some(Secret::new("t0k")));
        assert_eq!(
            Settings::load(&paths.config_file())
                .unwrap()
                .0
                .secrets_backend,
            Some(Backend::EncryptedFile)
        );
        assert!(matches!(load(&mut session), Action::SecretsLoaded { .. }));

        // a store that is still locked asks first
        let mut fresh = Session::with(
            &paths,
            Box::new(|| Err(SecretError::Unavailable("none".into()))),
            Box::new(|_| Arc::new(Shut)),
        );

        assert!(matches!(
            migrate(&mut fresh, Backend::Keychain, None),
            Action::SecretsLocked {
                locked: crate::app::Locked::Passphrase { .. },
                ..
            }
        ));
    }

    #[test]
    fn approvals_go_to_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = session(dir.path(), true);

        assert!(matches!(
            session.run(Job::Approve {
                id: 3,
                keys: vec![token()],
                origin: "https://h/".into(),
            }),
            Action::Approved {
                id: 3,
                result: Ok(())
            }
        ));
        assert_eq!(
            SecretIndex::new(Paths::under(dir.path()).secrets_index_file())
                .unapproved(&[token()], "https://h/")
                .unwrap(),
            None
        );
        assert!(matches!(
            session.run(Job::Approve {
                id: 4,
                keys: vec![token()],
                origin: "no origin".into(),
            }),
            Action::Approved { result: Err(_), .. }
        ));
    }
}
