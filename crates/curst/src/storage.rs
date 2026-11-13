//! runs the store commands against the data folder and reports the result
//! back as an action; every change reloads the collection it touched

use curst_core::model::Collection;
use curst_core::store::{CollectionDir, CollectionStore, Library, LoadIssue, StoreError};

use crate::app::{Action, Change, Command, NodeKey, Stored, Workspace};

fn describe(issues: Vec<LoadIssue>) -> Vec<String> {
    issues
        .into_iter()
        .map(|i| format!("{}: {}", i.path.display(), i.message))
        .collect()
}

/// a collection with its local id and remembered environment, plus the
/// files that could not be read
fn open(library: &Library, dir: &CollectionDir) -> Result<(Workspace, Vec<String>), StoreError> {
    let entry = library.registry().register(dir.root(), true)?;
    let loaded = dir.load()?;
    let workspace = workspace(entry.id, dir, loaded.collection, entry.active_environment);

    Ok((workspace, describe(loaded.issues)))
}

fn workspace(
    id: curst_core::model::LocalId,
    dir: &CollectionDir,
    collection: Collection,
    active: Option<curst_core::model::EnvId>,
) -> Workspace {
    // an environment deleted since it was chosen counts as none
    let active_environment = active.filter(|id| collection.environment(id).is_some());

    Workspace {
        id,
        root: dir.root().to_owned(),
        collection,
        active_environment,
    }
}

/// every collection and the globals, for startup
pub(crate) fn load_all(library: &Library) -> Action {
    let mut problems = Vec::new();
    let mut collections = Vec::new();

    match library.collections() {
        Ok(dirs) => {
            for dir in dirs {
                match open(library, &dir) {
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

    let globals = library.globals().unwrap_or_else(|err| {
        problems.push(err.to_string());

        Vec::new()
    });

    Action::Loaded {
        collections,
        globals,
        problems,
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
                .and_then(|dir| open(library, &dir))
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
        Command::Send { .. } | Command::Cancel | Command::SaveBody(_) => None,
    }
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
        Change::RenameCollection(name) => {
            let renamed = library.rename_collection(&dir, &name)?;
            let variables = renamed.load()?.collection.variables;

            renamed.save_settings(&name, &variables)?;
            dir = renamed;
        }
        Change::DeleteCollection => {
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

    stored.workspace = Some(self::workspace(
        workspace.id.clone(),
        &dir,
        loaded.collection,
        workspace.active_environment.clone(),
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

    fn loaded(library: &Library) -> (Vec<Workspace>, Vec<String>) {
        let Action::Loaded {
            collections,
            problems,
            ..
        } = load_all(library)
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
        let (pets, _) = open(&library, &dir).unwrap();
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
        let (pets, _) = open(&library, &dir).unwrap();
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
        let (pets, _) = open(&library, &dir).unwrap();
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
