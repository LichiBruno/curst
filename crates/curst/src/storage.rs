//! runs the store commands against the data folder and reports the result
//! back as an action; every change reloads the collection it touched

use std::path::{Path, PathBuf};

use curst_core::model::{Collection, EnvId, LocalId, Source};
use curst_core::openapi;
use curst_core::store::{CollectionDir, CollectionStore, Entry, Library, LoadIssue, StoreError};

use crate::app::{Action, Change, Command, NodeKey, SpecFound, SpecPreview, Stored, Workspace};

fn describe(issues: Vec<LoadIssue>) -> Vec<String> {
    issues
        .into_iter()
        .map(|i| format!("{}: {}", i.path.display(), i.message))
        .collect()
}

/// a collection with its local id, trust and remembered environment, plus
/// the problems found in its files; `trust_new` is the trust it gets when
/// this machine has never seen it
pub(crate) fn open(
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
        entry.auto_save,
        managed,
    )
}

fn rebuilt(
    id: LocalId,
    dir: &CollectionDir,
    collection: Collection,
    active: Option<EnvId>,
    trusted: bool,
    auto_save: bool,
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
        auto_save,
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

/// a path as the user typed it, `~` for the home folder, made absolute so
/// it still works when curst starts somewhere else
fn absolute(typed: &str) -> PathBuf {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    let expanded = match (typed.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(typed),
    };

    std::path::absolute(&expanded).unwrap_or(expanded)
}

/// the spec at `location`, read and converted for a preview
fn read_spec(location: &str, picked: Option<&str>) -> Result<SpecFound, String> {
    let location = location.trim();

    if location.is_empty() {
        return Err("give a file, a folder or a URL".into());
    }

    let path = absolute(location);

    if !path.exists() {
        return Err(format!("there is nothing at {}", path.display()));
    }

    // a folder: the spec in it, the one picked, or a choice among them
    let (file, picked) = if path.is_dir() {
        let specs = openapi::specs_in(&path);
        let chosen = match (picked, specs.len()) {
            // only a spec found there, so a name cannot lead elsewhere
            (Some(name), _) if specs.iter().any(|s| s == name) => name.to_owned(),
            (Some(name), _) => {
                return Err(format!("{name} is no longer a spec in {}", path.display()));
            }
            (None, 0) => {
                return Err(format!(
                    "no OpenAPI or Swagger spec is in {} or the folders under it",
                    path.display()
                ));
            }
            (None, 1) => specs[0].clone(),
            (None, _) => {
                return Ok(SpecFound::Choose {
                    folder: path.display().to_string(),
                    specs,
                });
            }
        };

        (path.join(&chosen), Some(chosen))
    } else {
        (path.clone(), None)
    };

    let text = openapi::read_file(&file).map_err(|e| e.to_string())?;
    let value = openapi::parse(&text).map_err(|e| e.to_string())?;
    let converted = openapi::convert(&value, None).map_err(|e| e.to_string())?;

    Ok(SpecFound::Preview(Box::new(SpecPreview::new(
        converted,
        Source {
            location: path.display().to_string(),
            picked,
            ..Source::default()
        },
    ))))
}

/// collections in and out of curst: bundles and specs
fn transfer(library: &Library, collections: &[Workspace], command: Command) -> Option<Action> {
    match command {
        Command::ExportBundle { collection, path } => {
            let Some(workspace) = collections.iter().find(|w| w.id == collection) else {
                return Some(Action::Notice("that collection is gone".into()));
            };

            let result = curst_core::store::bundle::export(&workspace.collection, &path)
                .map(|()| path)
                .map_err(|e| e.to_string());

            Some(Action::Exported(result))
        }
        Command::ImportBundle(path) => {
            let result = library
                .import_bundle(&path)
                .and_then(|(dir, bundle)| {
                    open(library, &dir, true)
                        .map(|(workspace, _)| (Box::new(workspace), bundle.warnings))
                })
                .map_err(|e| e.to_string());

            Some(Action::Imported(result))
        }
        Command::ReadSpec { location, picked } => {
            Some(Action::SpecRead(read_spec(&location, picked.as_deref())))
        }
        Command::ImportSpec(preview) => {
            let result = library
                .import_spec(preview.converted, &preview.source)
                .and_then(|(dir, bundle)| {
                    open(library, &dir, true)
                        .map(|(workspace, _)| (Box::new(workspace), bundle.warnings))
                })
                .map_err(|e| e.to_string());

            Some(Action::Imported(result))
        }
        _ => None,
    }
}

/// keeps in the registry a choice this user made for a collection
fn remember(library: &Library, workspace: &Workspace, command: Command) -> Option<Action> {
    let registry = library.registry();
    let result = match command {
        Command::SetAutoSave { on, .. } => registry.set_auto_save(&workspace.root, on),
        Command::SetActiveEnvironment { environment, .. } => {
            registry.set_active_environment(&workspace.root, environment.as_ref())
        }
        _ => Ok(()),
    };

    result
        .err()
        .map(|e| Action::Notice(format!("could not remember that: {e}")))
}

/// has every request inherit its auth, the collection's set to `auth` first
fn inherit_auth(
    dir: &CollectionDir,
    collection: &curst_core::model::Collection,
    auth: Option<curst_core::model::Auth>,
) -> Result<(), StoreError> {
    use curst_core::model::{Auth, Item};

    if let Some(auth) = auth {
        dir.save_auth(&[], &auth)?;
    }

    for path in collection.own_auth() {
        if let Some(Item::Request(saved)) = collection.item(&path) {
            let request = curst_core::model::Request {
                auth: Auth::Inherit,
                ..saved.request.clone()
            };

            dir.save_request(&path, &request)?;
        }
    }

    Ok(())
}

/// records where the collection's spec is; a path is kept absolute, so it
/// works wherever curst starts
fn set_source(
    dir: &CollectionDir,
    mut source: curst_core::model::Source,
) -> Result<(), StoreError> {
    if !crate::app::is_url(&source.location) {
        source.location = absolute(&source.location).display().to_string();
    }

    dir.save_source(Some(&source))
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
        Command::SetAutoSave { ref collection, .. }
        | Command::SetActiveEnvironment { ref collection, .. } => {
            let workspace = find(collection)?;

            remember(library, workspace, command)
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
        Command::ExportBundle { .. }
        | Command::ImportBundle(_)
        | Command::ReadSpec { .. }
        | Command::ImportSpec(_) => transfer(library, collections, command),
        Command::SaveSidebarWidth(width) => {
            let path = library.paths().config_file();

            curst_core::store::Settings::set_sidebar_width(&path, width)
                .err()
                .map(|err| Action::Notice(format!("settings not saved: {err}")))
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
        | Command::SendStep { .. }
        | Command::CancelStep
        | Command::Cancel
        | Command::SaveBody(_)
        | Command::LoadSecrets { .. }
        | Command::Unlock(_)
        | Command::ChooseBackend(_)
        | Command::Approve { .. }
        | Command::ChangeSecrets { .. }
        | Command::Migrate { .. }
        | Command::EditBody { .. }
        | Command::FetchSpec { .. }
        | Command::LoadSpec { .. }
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
                stored.opened = Some((created.clone(), request));
            }

            stored.created = Some(created);
        }
        Change::SaveRequest {
            path,
            request,
            quiet,
        } => {
            dir.save_request(&path, &request)?;
            stored.saved = Some((key(path), request));
            stored.quiet = quiet;
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
        Change::ApplyRefresh { plan, choices } => {
            curst_core::openapi::refresh::apply(&dir, &workspace.collection, &plan, &choices)?;
        }
        Change::SaveAuth { path, auth } => dir.save_auth(&path, &auth)?,
        Change::SaveWorkflows(workflows) => dir.save_workflows(&workflows)?,
        Change::InheritAuth { auth } => inherit_auth(&dir, &workspace.collection, auth)?,
        Change::SetSource(source) => set_source(&dir, source)?,
    }

    let loaded = dir.load()?;

    stored.workspace = Some(rebuilt(
        workspace.id.clone(),
        &dir,
        loaded.collection,
        workspace.active_environment.clone(),
        workspace.trusted,
        workspace.auto_save,
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

    const STORE_SPEC: &str = include_str!("../../curst-core/src/openapi/fixtures/store-3.0.json");
    const NOTES_SPEC: &str = include_str!("../../curst-core/src/openapi/fixtures/notes-3.1.yaml");

    fn read(library: &Library, location: &str, picked: Option<&str>) -> Result<SpecFound, String> {
        let command = Command::ReadSpec {
            location: location.to_owned(),
            picked: picked.map(ToOwned::to_owned),
        };

        match run(library, &[], command) {
            Some(Action::SpecRead(result)) => result,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_spec_file_is_previewed_then_imported_with_its_source() {
        let (dir, library) = library();
        let file = dir.path().join("store.json");

        std::fs::write(&file, STORE_SPEC).unwrap();

        let Ok(SpecFound::Preview(preview)) = read(&library, &file.display().to_string(), None)
        else {
            panic!("not read");
        };

        assert_eq!(preview.converted.requests, 4);
        assert_eq!(preview.source.location, file.display().to_string());

        // nothing is written until the preview is accepted
        assert_eq!(library.collections().unwrap().len(), 0);

        let Some(Action::Imported(Ok((workspace, _)))) =
            run(&library, &[], Command::ImportSpec(preview))
        else {
            panic!("not imported");
        };

        assert_eq!(workspace.collection.name, "Store");
        assert_eq!(
            workspace
                .collection
                .source
                .as_ref()
                .map(|s| s.location.clone()),
            Some(file.display().to_string())
        );

        // what is not a spec, or not there, says why
        assert!(read(&library, "/no/such/spec.yaml", None).is_err());

        std::fs::write(&file, "just: yaml\n").unwrap();

        let Err(message) = read(&library, &file.display().to_string(), None) else {
            panic!("read");
        };

        assert!(message.contains("not an OpenAPI"), "{message}");
    }

    mod refresh {
        use curst_core::openapi;

        use super::*;
        use crate::app::{Popup, State, update};

        /// the store spec a while later: an endpoint added, the order schema
        /// changed, the health check gone
        fn newer() -> String {
            STORE_SPEC
                .replace(
                    "\"quantity\": { \"type\": \"integer\" }",
                    "\"quantity\": { \"type\": \"number\" }",
                )
                .replace(
                    "\"/health\": {\n      \"get\": { \"security\": [], \"responses\": { \"200\": { \"description\": \"up\" } } }\n    }",
                    "\"/customers/{customerId}/orders\": {\n      \"get\": { \"tags\": [\"customers\"], \"summary\": \"Orders of a customer\", \"responses\": { \"200\": { \"description\": \"ok\" } } }\n    }",
                )
        }

        /// a state holding the store spec, imported from `file`
        fn imported(library: &Library, file: &Path) -> State {
            let Ok(SpecFound::Preview(preview)) = read(library, &file.display().to_string(), None)
            else {
                panic!("not read");
            };
            let Some(Action::Imported(Ok((workspace, _)))) =
                run(library, &[], Command::ImportSpec(preview))
            else {
                panic!("not imported");
            };
            let mut state = State::default();

            update(&mut state, Action::Imported(Ok((workspace, Vec::new()))));
            state.popup = None;

            state
        }

        /// what the runtime does with a command: storage runs it, and its
        /// answer goes back to the app
        fn round_trip(library: &Library, state: &mut State, command: Command) -> Vec<Command> {
            let action = run(library, &state.collections, command).unwrap();

            update(state, action)
        }

        /// the spec loaded again, as the worker would
        fn load(state: &mut State, commands: &[Command]) {
            let [
                Command::LoadSpec {
                    id,
                    collection,
                    source,
                    ..
                },
            ] = commands
            else {
                panic!("{commands:?}");
            };
            let offline = |_: &str| -> Result<openapi::Fetched, String> { Err("offline".into()) };
            let result = openapi::refresh::load_source(source, offline)
                .map(|(value, at)| Box::new(openapi::convert(&value, at.as_deref()).unwrap()));

            update(
                state,
                Action::SpecLoaded {
                    id: *id,
                    collection: collection.clone(),
                    result,
                },
            );
        }

        fn has(state: &State, operation: &str) -> bool {
            fn walk(items: &[curst_core::model::Item], operation: &str) -> bool {
                items.iter().any(|item| match item {
                    curst_core::model::Item::Folder(f) => walk(&f.items, operation),
                    curst_core::model::Item::Request(r) => {
                        r.spec.as_ref().is_some_and(|s| s.operation == operation)
                    }
                })
            }

            walk(&state.collections[0].collection.items, operation)
        }

        fn screen(state: &State) -> String {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 30)).unwrap();

            terminal
                .draw(|frame| crate::ui::view(state, frame))
                .unwrap();

            terminal.backend().to_string()
        }

        fn key(path: &[String], state: &State) -> NodeKey {
            NodeKey {
                collection: state.collections[0].id.clone(),
                path: path.to_vec(),
            }
        }

        #[test]
        fn new_changed_and_gone_endpoints_are_offered_and_the_chosen_applied() {
            let (dir, library) = library();
            let file = dir.path().join("store.json");

            std::fs::write(&file, STORE_SPEC).unwrap();

            let mut state = imported(&library, &file);
            let create = vec!["orders".to_owned(), "create-order".to_owned()];

            // the request about to change is open, unedited
            state.load_request(&key(&create, &state));

            // nothing changed yet: up to date
            let commands = update(&mut state, Action::RefreshSpec);

            load(&mut state, &commands);

            assert!(state.popup.is_none());
            assert!(
                state.notice.as_deref().unwrap().contains("up to date"),
                "{:?}",
                state.notice
            );

            // the spec changes
            std::fs::write(&file, newer()).unwrap();

            let commands = update(&mut state, Action::RefreshSpec);

            load(&mut state, &commands);

            let Some(Popup::Review(review)) = &state.popup else {
                panic!("no review: {:?}", state.notice);
            };

            // the new one is taken, the change and the gone one left
            assert_eq!(review.chosen, [true, false, false]);
            assert!(review.label(1).starts_with("changed  POST /orders"));

            let changed = review.plan.changed[0].clone();

            update(&mut state, Action::Down);

            insta::assert_snapshot!(screen(&state));

            update(&mut state, Action::ToggleRow);

            let mut commands = update(&mut state, Action::ConfirmPopup);

            assert!(
                matches!(commands.as_slice(), [Command::Store { .. }]),
                "{commands:?}"
            );
            round_trip(&library, &mut state, commands.remove(0));

            assert_eq!(
                state.notice.as_deref(),
                Some("refreshed: 1 added, 1 replaced, 0 deleted")
            );
            assert!(has(&state, "GET /customers/{customerId}/orders"));
            assert!(has(&state, "GET /health"));

            // the tab that showed it shows what the spec says now
            assert_eq!(state.request(), changed.new);
            assert!(!state.is_dirty());

            // what was left is offered again, nothing else
            let commands = update(&mut state, Action::RefreshSpec);

            load(&mut state, &commands);

            let Some(Popup::Review(review)) = &state.popup else {
                panic!("no review");
            };

            assert_eq!(review.lines.len(), 1);
            assert!(review.label(0).starts_with("gone"));
        }

        #[test]
        fn a_request_with_unsaved_edits_is_held_back() {
            let (dir, library) = library();
            let file = dir.path().join("store.json");

            std::fs::write(&file, STORE_SPEC).unwrap();

            let mut state = imported(&library, &file);
            let create = vec!["orders".to_owned(), "create-order".to_owned()];

            state.load_request(&key(&create, &state));
            state.url.insert_str("?mine");

            std::fs::write(&file, newer()).unwrap();

            let commands = update(&mut state, Action::RefreshSpec);

            load(&mut state, &commands);

            let Some(Popup::Review(review)) = &state.popup else {
                panic!("no review");
            };

            assert_eq!(review.held, [false, true, false]);
            assert!(review.label(1).contains("unsaved edits"));

            // it cannot be taken
            update(&mut state, Action::Down);
            update(&mut state, Action::ToggleRow);

            let Some(Popup::Review(review)) = &state.popup else {
                panic!("no review");
            };

            assert!(!review.chosen[1]);
            assert!(
                state
                    .notice
                    .as_deref()
                    .unwrap()
                    .contains("save or close it")
            );
        }

        #[test]
        fn a_spec_that_moved_is_asked_for_and_refreshed_from_there() {
            let (dir, library) = library();
            let file = dir.path().join("store.json");
            let moved = dir.path().join("moved.json");

            std::fs::write(&file, STORE_SPEC).unwrap();

            let mut state = imported(&library, &file);

            std::fs::rename(&file, &moved).unwrap();

            let commands = update(&mut state, Action::RefreshSpec);

            load(&mut state, &commands);

            let Some(Popup::Prompt { input, .. }) = &mut state.popup else {
                panic!("not asked: {:?}", state.notice);
            };

            // the old place is offered to edit
            assert_eq!(crate::app::editor_text(input), file.display().to_string());

            input.select_all();
            input.cut();
            input.insert_str(moved.display().to_string());

            let mut commands = update(&mut state, Action::ConfirmPopup);

            assert_eq!(commands.len(), 1, "{commands:?}");

            // once the new place is saved, the refresh runs from there
            let commands = round_trip(&library, &mut state, commands.remove(0));

            assert!(matches!(
                commands.as_slice(),
                [Command::LoadSpec { source, .. }] if source.location == moved.display().to_string()
            ));

            // and the source can be changed at any time, kept absolute
            state.focus = crate::app::Focus::Sidebar;
            update(&mut state, Action::ChangeSource);

            let Some(Popup::Prompt { input, .. }) = &mut state.popup else {
                panic!("not asked");
            };

            input.select_all();
            input.cut();
            input.insert_str("https://api.example.com/docs");

            let mut commands = update(&mut state, Action::ConfirmPopup);

            round_trip(&library, &mut state, commands.remove(0));

            assert_eq!(
                state.collections[0]
                    .collection
                    .source
                    .as_ref()
                    .map(|s| s.location.as_str()),
                Some("https://api.example.com/docs")
            );
        }
    }

    #[test]
    fn a_folder_gives_its_spec_or_a_choice_among_them() {
        let (dir, library) = library();
        let folder = dir.path().join("repo");
        let folder_text = folder.display().to_string();

        std::fs::create_dir_all(folder.join("api")).unwrap();

        // nothing in it says so
        let Err(message) = read(&library, &folder_text, None) else {
            panic!("read");
        };

        assert!(message.contains("no OpenAPI"), "{message}");

        // one spec is read at once, and remembered as the one picked
        std::fs::write(folder.join("api/openapi.yaml"), NOTES_SPEC).unwrap();

        let Ok(SpecFound::Preview(preview)) = read(&library, &folder_text, None) else {
            panic!("not read");
        };
        let notes = format!("api{}openapi.yaml", std::path::MAIN_SEPARATOR);

        assert_eq!(preview.source.location, folder_text);
        assert_eq!(preview.source.picked.as_deref(), Some(notes.as_str()));

        // several: a choice, then the one chosen
        std::fs::write(folder.join("store.json"), STORE_SPEC).unwrap();

        let Ok(SpecFound::Choose { specs, .. }) = read(&library, &folder_text, None) else {
            panic!("no choice");
        };

        assert_eq!(specs, ["store.json".to_owned(), notes.clone()]);

        let Ok(SpecFound::Preview(preview)) = read(&library, &folder_text, Some("store.json"))
        else {
            panic!("not read");
        };

        assert_eq!(preview.converted.bundle.name, "Store");

        // a name that is not one of its specs leads nowhere else
        std::fs::write(dir.path().join("outside.json"), STORE_SPEC).unwrap();

        assert!(read(&library, &folder_text, Some("../outside.json")).is_err());
        assert!(read(&library, &folder_text, Some("gone.json")).is_err());
    }

    #[test]
    fn a_typed_path_is_made_absolute() {
        let path = absolute("specs/api.yaml");

        assert!(path.is_absolute());
        assert!(path.ends_with("specs/api.yaml"));

        if std::env::var_os("HOME").is_some() {
            assert!(!absolute("~/api.yaml").starts_with("~"));
        }
    }

    #[test]
    fn a_collection_goes_out_as_a_bundle_and_comes_back_as_another() {
        let (dir, library) = library();
        let created = library.create_collection("Pets").unwrap();
        let (pets, _) = open(&library, &created, true).unwrap();
        let path = dir.path().join("pets.curst.toml");
        let exported = run(
            &library,
            std::slice::from_ref(&pets),
            Command::ExportBundle {
                collection: pets.id.clone(),
                path: path.clone(),
            },
        );

        assert!(matches!(exported, Some(Action::Exported(Ok(p))) if p == path));

        // a second export would overwrite it, so it is refused
        assert!(matches!(
            run(
                &library,
                std::slice::from_ref(&pets),
                Command::ExportBundle {
                    collection: pets.id.clone(),
                    path: path.clone(),
                },
            ),
            Some(Action::Exported(Err(_)))
        ));

        let Some(Action::Imported(Ok((imported, warnings)))) =
            run(&library, &[], Command::ImportBundle(path))
        else {
            panic!("not imported");
        };

        assert_eq!(imported.collection.name, "Pets");
        assert_ne!(imported.id, pets.id);
        assert!(imported.trusted);
        assert_eq!(warnings, Vec::<String>::new());
        assert!(matches!(
            run(
                &library,
                &[],
                Command::ImportBundle(dir.path().join("none.toml"))
            ),
            Some(Action::Imported(Err(_)))
        ));
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
        assert_eq!(created.opened.as_ref().map(|(k, _)| k), Some(&key));

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
    fn auto_save_is_remembered_and_survives_a_save() {
        let (_dir, library) = library();
        let dir = library.create_collection("Pets").unwrap();
        let (mut pets, _) = open(&library, &dir, true).unwrap();
        let none = run(
            &library,
            std::slice::from_ref(&pets),
            Command::SetAutoSave {
                collection: pets.id.clone(),
                on: true,
            },
        );

        assert!(none.is_none());
        assert!(loaded(&library).0[0].auto_save);

        // the app turns it on in memory as it asks for it
        pets.auto_save = true;

        let pets = change(
            &library,
            &pets,
            Change::CreateFolder {
                parent: vec![],
                name: "users".into(),
            },
        )
        .workspace
        .unwrap();

        assert!(pets.auto_save);
    }

    #[test]
    fn every_request_comes_to_inherit_the_collection_auth() {
        use curst_core::model::Auth;

        let (_dir, library) = library();
        let dir = library.create_collection("Pets").unwrap();
        let own = Request {
            url: "http://h/".into(),
            auth: Auth::None,
            ..Request::default()
        };

        dir.create_request(&[], "One", &own).unwrap();
        dir.create_request(&[], "Two", &own).unwrap();

        let (pets, _) = open(&library, &dir, true).unwrap();
        let bearer = Auth::Bearer {
            token: "{{api_key}}".into(),
        };
        let pets = change(
            &library,
            &pets,
            Change::InheritAuth {
                auth: Some(bearer.clone()),
            },
        )
        .workspace
        .unwrap();

        assert_eq!(pets.collection.auth, bearer);
        assert_eq!(pets.collection.own_auth(), Vec::<Vec<String>>::new());
        assert!(matches!(
            pets.collection.item(&["one".to_owned()]),
            Some(Item::Request(r)) if r.request.url == "http://h/"
        ));
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
