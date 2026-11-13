//! runs the store commands against the data folder and reports the result
//! back as an action; every change reloads the collection it touched

use std::path::{Path, PathBuf};

use curst_core::model::{Collection, EnvId, LocalId};
use curst_core::store::{CollectionDir, CollectionStore, Entry, Library, LoadIssue, StoreError};

use crate::app::{Action, Change, Command, NodeKey, Stored, Workspace};

fn describe(issues: Vec<LoadIssue>) -> Vec<String> {
    issues
        .into_iter()
        .map(|i| format!("{}: {}", i.path.display(), i.message))
        .collect()
}

/// a collection with its local id, trust and remembered environment, plus
/// the problems found in its files; `trust_new` is the trust it gets when
/// this machine has never seen it
fn open(
    library: &Library,
    dir: &CollectionDir,
    trust_new: bool,
) -> Result<(Workspace, Vec<String>), StoreError> {
    let entry = library.registry().register(dir.root(), trust_new)?;
    let loaded = dir.load()?;
    let workspace = workspace(&entry, dir, loaded.collection, library.is_managed(dir));

    Ok((workspace, describe(loaded.issues)))
}

fn workspace(
    entry: &Entry,
    dir: &CollectionDir,
    collection: Collection,
    managed: bool,
) -> Workspace {
    rebuilt(
        entry.id.clone(),
        dir,
        collection,
        entry.active_environment.clone(),
        entry.trusted,
        managed,
    )
}

fn rebuilt(
    id: LocalId,
    dir: &CollectionDir,
    collection: Collection,
    active: Option<EnvId>,
    trusted: bool,
    managed: bool,
) -> Workspace {
    // an environment deleted since it was chosen counts as none
    let active_environment = active.filter(|id| collection.environment(id).is_some());

    Workspace {
        id,
        root: dir.root().to_owned(),
        collection,
        active_environment,
        trusted,
        managed,
    }
}

fn same_folder(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// every collection and the globals, for startup: those in the data folder,
/// then the `.curst/` folder in `cwd`, which is untrusted until the user
/// says otherwise
pub(crate) fn load_all(library: &Library, cwd: Option<&Path>) -> Action {
    let mut problems = Vec::new();
    let mut collections = Vec::new();

    match library.collections() {
        Ok(dirs) => {
            for dir in dirs {
                match open(library, &dir, true) {
                    Ok((workspace, issues)) => {
                        collections.push(workspace);
                        problems.extend(issues);
                    }
                    Err(err) => problems.push(err.to_string()),
                }
            }
        }
        Err(err) => problems.push(err.to_string()),
    }

    if let Some(local) = cwd.and_then(Library::local_collection)
        && !collections
            .iter()
            .any(|w| same_folder(&w.root, local.root()))
    {
        match open(library, &local, false) {
            Ok((workspace, issues)) => {
                collections.push(workspace);
                problems.extend(issues);
            }
            Err(err) => problems.push(err.to_string()),
        }
    }

    // collections whose folder has gone, offered when trusting a new one
    let orphans: Vec<(LocalId, PathBuf)> = if collections.iter().all(|w| w.trusted) {
        Vec::new()
    } else {
        library.registry().orphans().map_or_else(
            |_| Vec::new(),
            |entries| entries.into_iter().map(|e| (e.id, e.path)).collect(),
        )
    };

    let globals = match library.globals() {
        Ok((globals, issues)) => {
            problems.extend(describe(issues));

            globals
        }
        Err(err) => {
            problems.push(err.to_string());

            Vec::new()
        }
    };

    Action::Loaded {
        collections,
        globals,
        problems,
        orphans,
    }
}

/// runs a store command, or returns `None` for one that is not about storage
pub(crate) fn run(
    library: &Library,
    collections: &[Workspace],
    command: Command,
) -> Option<Action> {
    let find = |id: &curst_core::model::LocalId| collections.iter().find(|w| w.id == *id);

    match command {
        Command::Store { collection, change } => {
            let Some(workspace) = find(&collection) else {
                return Some(Action::Notice("that collection is gone".into()));
            };

            let result = store(library, workspace, change)
                .map(Box::new)
                .map_err(|e| e.to_string());

            Some(Action::Stored { collection, result })
        }
        Command::CreateCollection(name) => {
            let result = library
                .create_collection(&name)
                .and_then(|dir| open(library, &dir, true))
                .map(|(workspace, _)| Box::new(workspace))
                .map_err(|e| e.to_string());

            Some(Action::CollectionAdded(result))
        }
        Command::SaveGlobals(variables) => {
            let result = library
                .save_globals(&variables)
                .map(|()| variables)
                .map_err(|e| e.to_string());

            Some(Action::GlobalsSaved(result))
        }
        Command::SetActiveEnvironment {
            collection,
            environment,
        } => {
            let workspace = find(&collection)?;

            library
                .registry()
                .set_active_environment(&workspace.root, environment.as_ref())
                .err()
                .map(|e| Action::Notice(format!("could not remember the environment: {e}")))
        }
        Command::Trust {
            collection,
            reattach,
        } => {
            let Some(workspace) = find(&collection) else {
                return Some(Action::Notice("that collection is gone".into()));
            };

            let result = trust(library, workspace, reattach.as_ref())
                .map(Box::new)
                .map_err(|e| e.to_string());

            Some(Action::Trusted {
                previous: collection,
                result,
            })
        }
        Command::SavePreferences { keys, theme } => {
            let path = library.paths().config_file();
            let result = keys
                .map_or(Ok(()), |keys| {
                    curst_core::store::Settings::set_keys_preset(&path, &keys)
                })
                .and_then(|()| {
                    theme.map_or(Ok(()), |theme| {
                        curst_core::store::Settings::set_theme(&path, &theme)
                    })
                });

            result
                .err()
                .map(|err| Action::Notice(format!("settings not saved: {err}")))
        }
        Command::Send { .. }
        | Command::Cancel
        | Command::SaveBody(_)
        | Command::LoadSecrets { .. }
        | Command::Unlock(_)
        | Command::ChooseBackend(_)
        | Command::Approve { .. }
        | Command::ChangeSecrets { .. }
        | Command::Migrate { .. }
        | Command::EditBody { .. }
        | Command::Copy(_) => None,
    }
}

/// marks a collection trusted, or gives it the local id and secrets of a
/// collection whose folder has gone
fn trust(
    library: &Library,
    workspace: &Workspace,
    reattach: Option<&LocalId>,
) -> Result<Workspace, StoreError> {
    let registry = library.registry();
    let entry = match reattach {
        Some(id) => registry.reattach(&workspace.root, id)?,
        None => registry.set_trusted(&workspace.root, true)?,
    };
    let dir = CollectionDir::open(&workspace.root);
    let loaded = dir.load()?;

    Ok(self::workspace(
        &entry,
        &dir,
        loaded.collection,
        workspace.managed,
    ))
}

fn store(library: &Library, workspace: &Workspace, change: Change) -> Result<Stored, StoreError> {
    let mut dir = CollectionDir::open(&workspace.root);
    let key = |path: Vec<String>| NodeKey {
        collection: workspace.id.clone(),
        path,
    };
    let joined = |parent: &[String], slug: String| {
        let mut path = parent.to_vec();

        path.push(slug);
        path
    };
    let mut stored = Stored::default();

    match change {
        Change::CreateRequest {
            parent,
            name,
            request,
            open,
        } => {
            let slug = dir.create_request(&parent, &name, &request)?;
            let created = key(joined(&parent, slug));

            if open {
                stored.saved = Some((created.clone(), request));
            }

            stored.created = Some(created);
        }
        Change::SaveRequest { path, request } => {
            dir.save_request(&path, &request)?;
            stored.saved = Some((key(path), request));
        }
        Change::CreateFolder { parent, name } => {
            let slug = dir.create_folder(&parent, &name)?;

            stored.created = Some(key(joined(&parent, slug)));
        }
        Change::Rename { path, name } => {
            let slug = dir.rename(&path, &name)?;
            let parent = &path[..path.len().saturating_sub(1)];

            stored.moved = Some((key(path.clone()), key(joined(parent, slug))));
        }
        Change::Move { path, parent } => {
            let slug = dir.move_item(&path, &parent)?;

            stored.moved = Some((key(path), key(joined(&parent, slug))));
        }
        Change::Reorder { path, index } => dir.reorder(&path, index)?,
        Change::Delete { path } => dir.delete(&path)?,
        // a folder curst did not make keeps its name; only the display
        // name changes
        Change::RenameCollection(name) => {
            if workspace.managed {
                dir = library.rename_collection(&dir, &name)?;
            }

            let variables = dir.load()?.collection.variables;

            dir.save_settings(&name, &variables)?;
        }
        Change::DeleteCollection => {
            if !workspace.managed {
                return Err(StoreError::Invalid(format!(
                    "curst does not delete {}; remove that folder yourself",
                    workspace.root.display()
                )));
            }

            library.delete_collection(&dir)?;

            return Ok(stored);
        }
        Change::SaveVariables(variables) => {
            dir.save_settings(&workspace.collection.name, &variables)?;
        }
        Change::CreateEnvironment(name) => {
            dir.create_environment(&name)?;
        }
        Change::SaveEnvironment(environment) => dir.save_environment(&environment)?,
        Change::RenameEnvironment { id, name } => {
            dir.rename_environment(&id, &name)?;
        }
        Change::DeleteEnvironment(id) => dir.delete_environment(&id)?,
    }

    let loaded = dir.load()?;

    stored.workspace = Some(rebuilt(
        workspace.id.clone(),
        &dir,
        loaded.collection,
        workspace.active_environment.clone(),
        workspace.trusted,
        workspace.managed,
    ));
    stored.problems = describe(loaded.issues);

    Ok(stored)
}

#[cfg(test)]
mod tests {
    use curst_core::model::{Item, Method, Request, Variable};
    use curst_core::store::Paths;
    use curst_core::vars::VarName;

    use super::*;

    fn library() -> (tempfile::TempDir, Library) {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::new(Paths::under(dir.path()));

        (dir, library)
    }

    #[test]
    fn preferences_are_written_to_the_config_file() {
        let (_dir, library) = library();
        let saved = run(
            &library,
            &[],
            Command::SavePreferences {
                keys: Some("vim".into()),
                theme: Some("light".into()),
            },
        );
        let (settings, _) =
            curst_core::store::Settings::load(&library.paths().config_file()).unwrap();

        assert!(saved.is_none());
        assert_eq!(settings.keys_preset.as_deref(), Some("vim"));
        assert_eq!(settings.theme.as_deref(), Some("light"));

        // only what is given changes
        run(
            &library,
            &[],
            Command::SavePreferences {
                keys: None,
                theme: Some("mono".into()),
            },
        );

        let (settings, _) =
            curst_core::store::Settings::load(&library.paths().config_file()).unwrap();

        assert_eq!(settings.keys_preset.as_deref(), Some("vim"));
        assert_eq!(settings.theme.as_deref(), Some("mono"));
    }

    fn loaded(library: &Library) -> (Vec<Workspace>, Vec<String>) {
        let Action::Loaded {
            collections,
            problems,
            ..
        } = load_all(library, None)
        else {
            panic!("not loaded");
        };

        (collections, problems)
    }

    fn stored(action: Option<Action>) -> Stored {
        match action {
            Some(Action::Stored { result, .. }) => *result.unwrap(),
            other => panic!("unexpected {other:?}"),
        }
    }

    fn change(library: &Library, workspace: &Workspace, change: Change) -> Stored {
        let command = Command::Store {
            collection: workspace.id.clone(),
            change,
        };

        stored(run(library, std::slice::from_ref(workspace), command))
    }

    fn path(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| (*p).to_owned()).collect()
    }

    #[test]
    fn an_empty_data_folder_loads_nothing() {
        let (_dir, library) = library();

        assert_eq!(loaded(&library), (Vec::new(), Vec::new()));
    }

    #[test]
    fn collections_are_created_changed_and_reloaded() {
        let (_dir, library) = library();
        let Some(Action::CollectionAdded(Ok(pets))) =
            run(&library, &[], Command::CreateCollection("Pets".into()))
        else {
            panic!("not created");
        };

        let request = Request {
            method: Method::Post,
            url: "{{base}}/pets".into(),
            ..Request::default()
        };
        let created = change(
            &library,
            &pets,
            Change::CreateRequest {
                parent: vec![],
                name: "Create pet".into(),
                request: request.clone(),
                open: true,
            },
        );
        let key = NodeKey {
            collection: pets.id.clone(),
            path: path(&["create-pet"]),
        };

        assert_eq!(created.created.as_ref(), Some(&key));
        assert_eq!(created.saved.as_ref().map(|(k, _)| k), Some(&key));

        let pets = created.workspace.unwrap();
        let folder = change(
            &library,
            &pets,
            Change::CreateFolder {
                parent: vec![],
                name: "Admin".into(),
            },
        );
        let pets = folder.workspace.unwrap();
        let moved = change(
            &library,
            &pets,
            Change::Move {
                path: path(&["create-pet"]),
                parent: path(&["admin"]),
            },
        );

        assert_eq!(
            moved.moved.as_ref().map(|(_, to)| to.path.clone()),
            Some(path(&["admin", "create-pet"]))
        );

        let pets = moved.workspace.unwrap();
        let renamed = change(
            &library,
            &pets,
            Change::Rename {
                path: path(&["admin", "create-pet"]),
                name: "Add pet".into(),
            },
        );

        assert_eq!(
            renamed.moved.map(|(_, to)| to.path),
            Some(path(&["admin", "add-pet"]))
        );

        let pets = renamed.workspace.unwrap();
        let Some(Item::Request(saved)) = pets.collection.item(&path(&["admin", "add-pet"])) else {
            panic!("no request");
        };

        assert_eq!(saved.request, request);

        // a fresh start finds the same collection under the same id
        let (collections, problems) = loaded(&library);

        assert_eq!(problems, Vec::<String>::new());
        assert_eq!(collections, [pets]);
    }

    #[test]
    fn environments_variables_and_the_active_one() {
        let (_dir, library) = library();
        let dir = library.create_collection("Pets").unwrap();
        let (pets, _) = open(&library, &dir, true).unwrap();
        let pets = change(&library, &pets, Change::CreateEnvironment("Dev".into()))
            .workspace
            .unwrap();
        let mut dev = pets.collection.environments[0].clone();
        let variable = Variable::new(VarName::new("base").unwrap(), "http://localhost");

        dev.variables = vec![variable.clone()];

        let pets = change(&library, &pets, Change::SaveEnvironment(dev.clone()))
            .workspace
            .unwrap();
        let pets = change(
            &library,
            &pets,
            Change::SaveVariables(vec![variable.clone()]),
        )
        .workspace
        .unwrap();

        assert_eq!(
            pets.collection.environments[0].variables,
            std::slice::from_ref(&variable)
        );
        assert_eq!(pets.collection.variables, std::slice::from_ref(&variable));

        let none = run(
            &library,
            std::slice::from_ref(&pets),
            Command::SetActiveEnvironment {
                collection: pets.id.clone(),
                environment: Some(dev.id.clone()),
            },
        );

        assert!(none.is_none());
        assert_eq!(
            loaded(&library).0[0].active_environment,
            Some(dev.id.clone())
        );

        let Some(Action::GlobalsSaved(Ok(globals))) =
            run(&library, &[], Command::SaveGlobals(vec![variable.clone()]))
        else {
            panic!("globals not saved");
        };

        assert_eq!(globals, [variable]);

        // the app sets the active environment in memory as it asks for it
        let mut pets = pets;

        pets.active_environment = Some(dev.id.clone());

        let renamed = change(
            &library,
            &pets,
            Change::RenameEnvironment {
                id: dev.id.clone(),
                name: "Local".into(),
            },
        );
        let pets = renamed.workspace.unwrap();

        assert_eq!(pets.environment_name(), Some("Local"));

        let pets = change(&library, &pets, Change::DeleteEnvironment(dev.id))
            .workspace
            .unwrap();

        assert_eq!(pets.active_environment, None);
        assert_eq!(loaded(&library).0[0].active_environment, None);
    }

    #[test]
    fn renaming_and_deleting_a_collection() {
        let (_dir, library) = library();
        let dir = library.create_collection("Pets").unwrap();
        let (pets, _) = open(&library, &dir, true).unwrap();
        let renamed = change(&library, &pets, Change::RenameCollection("Animals".into()))
            .workspace
            .unwrap();

        assert_eq!(renamed.id, pets.id);
        assert_eq!(renamed.collection.name, "Animals");
        assert!(renamed.root.ends_with("animals"));

        let deleted = change(&library, &renamed, Change::DeleteCollection);

        assert!(deleted.workspace.is_none());
        assert_eq!(loaded(&library).0, Vec::new());
    }

    #[test]
    fn failures_and_unrelated_commands() {
        let (_dir, library) = library();
        let dir = library.create_collection("Pets").unwrap();
        let (pets, _) = open(&library, &dir, true).unwrap();
        let missing = run(
            &library,
            std::slice::from_ref(&pets),
            Command::Store {
                collection: pets.id.clone(),
                change: Change::Delete {
                    path: path(&["nope"]),
                },
            },
        );

        assert!(matches!(
            missing,
            Some(Action::Stored { result: Err(_), .. })
        ));
        assert!(matches!(
            run(
                &library,
                &[],
                Command::Store {
                    collection: pets.id.clone(),
                    change: Change::DeleteCollection,
                },
            ),
            Some(Action::Notice(_))
        ));
        assert!(run(&library, &[], Command::Cancel).is_none());
        assert!(
            run(
                &library,
                &[],
                Command::SetActiveEnvironment {
                    collection: pets.id,
                    environment: None,
                },
            )
            .is_none()
        );

        std::fs::write(dir.root().join("broken.toml"), "name = ").unwrap();

        let (_, problems) = loaded(&library);

        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("broken.toml"));
    }
}

#[cfg(test)]
mod local_tests {
    use std::fs;

    use super::*;
    use curst_core::store::Paths;

    fn project() -> (tempfile::TempDir, Library, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::new(Paths::under(dir.path().join("home")));
        let project = dir.path().join("repo");

        CollectionDir::create(project.join(".curst"), "Shared").unwrap();
        library.create_collection("Mine").unwrap();

        (dir, library, project)
    }

    fn loaded(library: &Library, cwd: &Path) -> (Vec<Workspace>, Vec<(LocalId, PathBuf)>) {
        let Action::Loaded {
            collections,
            orphans,
            ..
        } = load_all(library, Some(cwd))
        else {
            panic!("not loaded");
        };

        (collections, orphans)
    }

    fn stored(action: Option<Action>) -> Result<Workspace, String> {
        match action {
            Some(Action::Stored { result, .. }) => result.map(|s| s.workspace.unwrap()),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn a_local_folder_stays_untrusted_until_trusted() {
        let (_dir, library, project) = project();
        let (collections, _) = loaded(&library, &project);
        let names: Vec<_> = collections
            .iter()
            .map(|w| (w.collection.name.as_str(), w.trusted, w.managed))
            .collect();

        assert_eq!(names, [("Mine", true, true), ("Shared", false, false)]);

        // a change to it, and a restart, keep it untrusted
        let shared = collections[1].clone();
        let changed = stored(run(
            &library,
            std::slice::from_ref(&shared),
            Command::Store {
                collection: shared.id.clone(),
                change: Change::CreateFolder {
                    parent: vec![],
                    name: "Admin".into(),
                },
            },
        ))
        .unwrap();

        assert!(!changed.trusted);
        assert!(!loaded(&library, &project).0[1].trusted);

        let Some(Action::Trusted { previous, result }) = run(
            &library,
            std::slice::from_ref(&shared),
            Command::Trust {
                collection: shared.id.clone(),
                reattach: None,
            },
        ) else {
            panic!("not trusted");
        };

        assert_eq!(previous, shared.id);
        assert!(result.unwrap().trusted);
        assert!(loaded(&library, &project).0[1].trusted);
    }

    #[test]
    fn a_moved_folder_can_take_over_its_old_secrets() {
        let (dir, library, project) = project();
        let (collections, orphans) = loaded(&library, &project);
        let old_id = collections[1].id.clone();

        assert_eq!(orphans, Vec::new());

        run(
            &library,
            &collections,
            Command::Trust {
                collection: old_id.clone(),
                reattach: None,
            },
        );

        // the folder is moved outside curst
        let moved = dir.path().join("moved-repo");

        fs::rename(&project, &moved).unwrap();

        let (collections, orphans) = loaded(&library, &moved);
        let fresh = collections[1].clone();

        assert!(!fresh.trusted);
        assert_ne!(fresh.id, old_id);
        assert_eq!(
            orphans.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            [&old_id]
        );

        let Some(Action::Trusted { result, .. }) = run(
            &library,
            std::slice::from_ref(&fresh),
            Command::Trust {
                collection: fresh.id.clone(),
                reattach: Some(old_id.clone()),
            },
        ) else {
            panic!("not trusted");
        };

        let taken = result.unwrap();

        assert_eq!((taken.id.clone(), taken.trusted), (old_id, true));
        assert_eq!(loaded(&library, &moved).0[1].id, taken.id);
        assert!(matches!(
            run(
                &library,
                &[],
                Command::Trust {
                    collection: taken.id,
                    reattach: None,
                },
            ),
            Some(Action::Notice(_))
        ));
    }

    #[test]
    fn a_folder_curst_did_not_make_is_never_renamed_or_deleted() {
        let (_dir, library, project) = project();
        let shared = loaded(&library, &project).0[1].clone();
        let one = std::slice::from_ref(&shared);
        let renamed = stored(run(
            &library,
            one,
            Command::Store {
                collection: shared.id.clone(),
                change: Change::RenameCollection("Team API".into()),
            },
        ))
        .unwrap();

        assert_eq!(renamed.collection.name, "Team API");
        assert_eq!(renamed.root, shared.root);

        let refused = stored(run(
            &library,
            std::slice::from_ref(&renamed),
            Command::Store {
                collection: shared.id.clone(),
                change: Change::DeleteCollection,
            },
        ));

        assert!(refused.unwrap_err().contains("does not delete"));
        assert!(project.join(".curst/collection.toml").exists());
    }
}
