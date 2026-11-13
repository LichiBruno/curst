//! pop-ups over the main screen: text prompts, yes/no questions, the
//! environment picker and the variable editor
//!
//! while one is open it receives every key but `Ctrl+C`

use std::path::PathBuf;

use curst_core::model::{EnvId, Environment, LocalId, Request, Variable};
use curst_core::secrets::detect::Place;
use curst_core::secrets::{Approval, Secret, SecretKey};
use curst_core::vars::VarName;
use ratatui_textarea::TextArea;

use super::editor::{Column, Row, Table, masked, single_line, text_of};
use super::workspace::NodeKey;
use super::{Action, Change, Command, Layer, SecretChange, State, step};

/// what a text prompt is for
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Purpose {
    SaveBody,
    SaveAs(NodeKey),
    NewRequest(NodeKey),
    NewFolder(NodeKey),
    NewCollection,
    Rename(NodeKey),
    NewEnvironment(LocalId),
    RenameEnvironment(LocalId, EnvId),

    /// the name of a new secret for the value at a place in the request
    MakeSecret(Place),
    ExportBundle(LocalId),
    ImportBundle,
    ImportSpec,
    ChangeSource(LocalId),

    /// where a spec that is not where it was is now, to refresh from it
    Relocate(LocalId),
}

impl Purpose {
    pub(crate) fn title(&self) -> &'static str {
        match self {
            Self::SaveBody => "Save body to file",
            Self::SaveAs(_) => "Save request as",
            Self::NewRequest(_) => "New request",
            Self::NewFolder(_) => "New folder",
            Self::NewCollection => "New collection",
            Self::Rename(_) => "Rename",
            Self::NewEnvironment(_) => "New environment",
            Self::RenameEnvironment(..) => "Rename environment",
            Self::MakeSecret(_) => "Name of the new secret",
            Self::ExportBundle(_) => "Export the collection to the file",
            Self::ImportBundle => "Import a bundle from the file",
            Self::ImportSpec => "Import an OpenAPI spec: a file, folder or URL",
            Self::ChangeSource(_) => "Where the spec of this collection is",
            Self::Relocate(_) => "The spec moved: where is it now?",
        }
    }
}

/// what a yes/no question does on yes
#[derive(Debug)]
pub(crate) enum Pending {
    Delete(NodeKey),
    DeleteEnvironment(LocalId, EnvId),
    /// quitting, with the pop-up to show again on no
    Quit(Option<Box<Popup>>),
    CloseTab,

    /// exporting a collection that holds plain-text credentials
    Export(LocalId, PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VarTab {
    Environment,
    Collection,
    Globals,
}

impl VarTab {
    pub(crate) const ALL: [Self; 3] = [Self::Environment, Self::Collection, Self::Globals];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Environment => "Environment",
            Self::Collection => "Collection",
            Self::Globals => "Globals",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Environment => 0,
            Self::Collection => 1,
            Self::Globals => 2,
        }
    }
}

/// the variables of one environment, one collection and the globals, each
/// in a table
#[derive(Debug)]
pub(crate) struct VariablesEditor {
    pub(crate) collection: LocalId,
    pub(crate) environment: Option<Environment>,
    pub(crate) tab: VarTab,
    pub(crate) tables: [Table; 3],
    baseline: [Vec<Row>; 3],
}

impl VariablesEditor {
    pub(crate) fn new(
        collection: LocalId,
        environment: Option<Environment>,
        collection_vars: &[Variable],
        globals: &[Variable],
    ) -> Self {
        let tables = [
            Table::from_variables(environment.as_ref().map_or(&[], |e| e.variables.as_slice())),
            Table::from_variables(collection_vars),
            Table::from_variables(globals),
        ];
        let baseline = tables.each_ref().map(|t| t.rows.clone());
        let tab = if environment.is_some() {
            VarTab::Environment
        } else {
            VarTab::Collection
        };

        Self {
            collection,
            environment,
            tab,
            tables,
            baseline,
        }
    }

    /// the tabs shown: the environment one only when an environment is
    /// being edited
    pub(crate) fn tabs(&self) -> Vec<VarTab> {
        VarTab::ALL
            .into_iter()
            .filter(|t| *t != VarTab::Environment || self.environment.is_some())
            .collect()
    }

    pub(crate) fn table(&self) -> &Table {
        &self.tables[self.tab.index()]
    }

    pub(crate) fn table_mut(&mut self) -> &mut Table {
        &mut self.tables[self.tab.index()]
    }

    pub(crate) fn editing(&self) -> bool {
        self.table().editing.is_some()
    }

    fn changed(&self, tab: VarTab) -> bool {
        self.tables[tab.index()].rows != self.baseline[tab.index()]
    }

    pub(crate) fn is_changed(&self) -> bool {
        VarTab::ALL.into_iter().any(|t| self.changed(t))
    }

    pub(crate) fn key(&self, tab: VarTab, name: VarName) -> Option<SecretKey> {
        match tab {
            VarTab::Environment => Some(SecretKey::Environment {
                collection: self.collection.clone(),
                environment: self.environment.as_ref()?.id.clone(),
                name,
            }),
            VarTab::Collection => Some(SecretKey::Collection {
                collection: self.collection.clone(),
                name,
            }),
            VarTab::Globals => Some(SecretKey::Global { name }),
        }
    }

    /// what saving a tab does to the secret store: a renamed secret moves,
    /// with the hosts it was approved for; a removed one is deleted; a new
    /// value is stored; names are checked before this is called
    fn secret_changes(&self, tab: VarTab) -> Vec<SecretChange> {
        let rows = &self.tables[tab.index()].rows;
        let before = &self.baseline[tab.index()];
        let key = |name: &str| VarName::new(name.trim()).and_then(|n| self.key(tab, n));
        let mut changes = Vec::new();

        for row in rows.iter().filter(|r| r.secret) {
            let Some(new) = key(&row.name) else {
                continue;
            };

            let was_secret = |old: &String| {
                before
                    .iter()
                    .any(|b| b.secret && b.loaded_name.as_ref() == Some(old))
            };

            if let Some(old) = row.loaded_name.as_ref().filter(|old| was_secret(old))
                && let Some(old) = key(old)
                && old != new
            {
                changes.push(SecretChange::Rename(old, new.clone()));
            }

            if let Some(value) = &row.pending {
                changes.push(SecretChange::Set(new, value.clone()));
            }
        }

        for gone in before.iter().filter(|b| b.secret) {
            let kept = rows
                .iter()
                .any(|r| r.secret && r.loaded_name == gone.loaded_name);

            if !kept && let Some(old) = key(&gone.name) {
                changes.push(SecretChange::Delete(old));
            }
        }

        changes
    }

    /// the variables of a tab, or the row and reason of the first bad one
    fn variables(&self, tab: VarTab) -> Result<Vec<Variable>, (usize, String)> {
        let mut variables: Vec<Variable> = Vec::new();

        for (row, pair) in self.tables[tab.index()].rows.iter().enumerate() {
            let Some(name) = VarName::new(pair.name.trim()) else {
                return Err((
                    row,
                    format!(
                        "{:?} is not a valid name: use letters, digits, _ - and .",
                        pair.name.trim()
                    ),
                ));
            };

            if variables.iter().any(|v| v.name == name) {
                return Err((row, format!("{name} is defined twice")));
            }

            variables.push(Variable {
                name,
                value: if pair.secret {
                    String::new()
                } else {
                    pair.value.clone()
                },
                enabled: pair.enabled,
                secret: pair.secret,
            });
        }

        Ok(variables)
    }
}

#[derive(Debug)]
pub(crate) enum Popup {
    Prompt {
        purpose: Purpose,
        input: Box<TextArea<'static>>,
    },
    Confirm {
        message: String,
        pending: Pending,
    },

    /// `selected` 0 is "no environment", then one per environment
    Environments {
        collection: LocalId,
        selected: usize,
    },
    Variables(Box<VariablesEditor>),

    /// whether to trust a collection that came from elsewhere, such as a
    /// `.curst/` folder in a repository
    Trust {
        collection: LocalId,
    },

    /// where secrets are kept, asked the first time one is needed
    ChooseBackend,

    /// the passphrase of the encrypted file; a new file asks twice, `first`
    /// holding the first entry
    Passphrase {
        new: bool,

        /// the passphrase of a file secrets are about to move into
        migrate: bool,
        first: Option<Secret>,
        input: Box<TextArea<'static>>,
        error: Option<String>,
    },

    /// whether secrets may go to an origin they were not sent to before
    Approve(Approval),

    /// where secrets are kept, and moving them
    Settings,

    /// the open tabs, to switch to, open or close one; it works in every
    /// terminal, where Ctrl with an arrow or a number may not arrive
    Tabs {
        selected: usize,
    },

    /// the first run: the keys, then the theme; `then` is a pop-up that
    /// was waiting and opens after
    Welcome {
        step: super::Step,
        then: Option<Box<Popup>>,
    },

    /// values for secrets that have none
    Fill(Box<super::fill::Fill>),

    /// whether to save the variables being left with changes; `quit` when
    /// curst quits after
    SaveVariables {
        editor: Box<VariablesEditor>,
        quit: bool,
    },

    /// what a spec becomes, before it is imported
    SpecPreview(Box<super::spec::Preview>),

    /// what a refresh found, to choose what to take
    Review(Box<super::refresh::Review>),

    /// the specs a folder holds, to choose one
    SpecChoice {
        folder: String,
        specs: Vec<String>,
        selected: usize,
    },

    /// actions and saved requests, found by typing
    Palette(Box<super::Palette>),

    /// every key; `here` is the chain of layers where the focus was
    Help {
        scroll: usize,
        here: Vec<Layer>,
    },
}

impl Popup {
    pub(crate) fn prompt(purpose: Purpose, text: &str) -> Self {
        let placeholder = match purpose {
            Purpose::SaveBody | Purpose::ExportBundle(_) => "file name",
            Purpose::ImportBundle => "path to the bundle file",
            Purpose::ImportSpec | Purpose::ChangeSource(_) | Purpose::Relocate(_) => {
                "a file, a folder, or a URL such as https://api.example.com/docs"
            }
            _ => "name",
        };

        Self::Prompt {
            purpose,
            input: Box::new(single_line(text, placeholder)),
        }
    }

    pub(crate) fn passphrase(new: bool, error: Option<String>) -> Self {
        Self::Passphrase {
            new,
            migrate: false,
            first: None,
            input: Box::new(masked("passphrase")),
            error,
        }
    }

    pub(crate) fn confirm(message: impl Into<String>, pending: Pending) -> Self {
        Self::Confirm {
            message: message.into(),
            pending,
        }
    }
}

/// handles `action` while a pop-up is open, or returns it when the pop-up
/// has no use for it
pub(crate) fn apply(state: &mut State, action: Action) -> Result<Vec<Command>, Action> {
    let Some(mut popup) = state.popup.take() else {
        return Err(action);
    };

    // a paste goes into the pop-up's field, never to what is under it
    if let Action::PasteText(text) = &action {
        paste(&mut popup, text);
        state.popup = Some(popup);

        return Ok(Vec::new());
    }

    match popup {
        Popup::Prompt { purpose, mut input } => match action {
            Action::Type(key) => {
                input.input(key);
                state.popup = Some(Popup::Prompt { purpose, input });
            }
            Action::Place { row, col, .. } => {
                super::mouse::place(&mut input, row, col);
                state.popup = Some(Popup::Prompt { purpose, input });
            }
            Action::ClosePopup => {}
            Action::ConfirmPopup => {
                let text = text_of(&input).trim().to_owned();

                if text.is_empty() {
                    state.popup = Some(Popup::Prompt { purpose, input });
                } else {
                    return Ok(confirm_prompt(state, purpose, input, text));
                }
            }
            other => {
                state.popup = Some(Popup::Prompt { purpose, input });

                return Err(other);
            }
        },
        Popup::Confirm { message, pending } => match action {
            Action::ConfirmPopup => return Ok(run_pending(state, pending)),
            Action::ClosePopup => {
                if let Pending::Quit(previous) = pending {
                    state.popup = previous.map(|p| *p);
                }
            }
            other => {
                state.popup = Some(Popup::Confirm { message, pending });

                return Err(other);
            }
        },
        Popup::Environments {
            collection,
            selected,
        } => return environments(state, collection, selected, action),
        Popup::Variables(editor) => return variables(state, editor, action),
        Popup::Trust { collection } => return trust(state, collection, action),
        popup @ (Popup::ChooseBackend | Popup::Passphrase { .. } | Popup::Approve(_)) => {
            return secret_popup(state, popup, action);
        }
        Popup::Tabs { selected } => return tab_switcher(state, selected, action),
        Popup::Help { scroll, here } => return help(state, scroll, here, action),
        Popup::Fill(fill) => return super::fill::apply(state, fill, action),
        Popup::SaveVariables { editor, quit } => {
            return save_question(state, editor, quit, action);
        }
        Popup::SpecPreview(preview) => return super::spec::preview_key(state, preview, action),
        Popup::Review(review) => return super::refresh::review_key(state, review, action),
        Popup::SpecChoice {
            folder,
            specs,
            selected,
        } => return super::spec::choice_key(state, folder, specs, selected, action),
        Popup::Palette(palette) => return super::palette::apply(state, palette, action),
        Popup::Welcome { step, then } => {
            return super::preferences::welcome_key(state, step, then, action);
        }
        Popup::Settings => match action {
            Action::Migrate => return Ok(super::send::migrate(state)),
            Action::NextKeys
            | Action::NextTheme
            | Action::SidebarNarrower
            | Action::SidebarWider => {
                state.popup = Some(Popup::Settings);

                return Ok(super::preferences::settings(state, &action).unwrap_or_default());
            }
            Action::ClosePopup => {}
            other => {
                state.popup = Some(Popup::Settings);

                return Err(other);
            }
        },
    }

    Ok(Vec::new())
}

fn confirm_prompt(
    state: &mut State,
    purpose: Purpose,
    input: Box<TextArea<'static>>,
    text: String,
) -> Vec<Command> {
    let store = |key: &NodeKey, change| Command::Store {
        collection: key.collection.clone(),
        change,
    };

    match purpose {
        Purpose::SaveBody => {
            // the prompt stays open until the file is written
            state.popup = Some(Popup::Prompt {
                purpose: Purpose::SaveBody,
                input,
            });

            vec![Command::SaveBody(PathBuf::from(text))]
        }
        Purpose::SaveAs(parent) => vec![store(
            &parent,
            Change::CreateRequest {
                parent: parent.path.clone(),
                name: text,
                request: state.request(),
                open: true,
            },
        )],
        Purpose::NewRequest(parent) => vec![store(
            &parent,
            Change::CreateRequest {
                parent: parent.path.clone(),
                name: text,
                request: Request::default(),
                open: false,
            },
        )],
        Purpose::NewFolder(parent) => vec![store(
            &parent,
            Change::CreateFolder {
                parent: parent.path.clone(),
                name: text,
            },
        )],
        Purpose::NewCollection => vec![Command::CreateCollection(text)],
        Purpose::ExportBundle(collection) => {
            super::sidebar::export(state, collection, PathBuf::from(text))
        }
        Purpose::ImportBundle => vec![Command::ImportBundle(PathBuf::from(text))],
        Purpose::ImportSpec => super::spec::start(state, &text),
        Purpose::ChangeSource(collection) => {
            super::refresh::set_source(state, collection, &text, false)
        }
        Purpose::Relocate(collection) => super::refresh::set_source(state, collection, &text, true),
        Purpose::Rename(key) if key.path.is_empty() => {
            vec![store(&key, Change::RenameCollection(text))]
        }
        Purpose::Rename(key) => vec![store(
            &key,
            Change::Rename {
                path: key.path.clone(),
                name: text,
            },
        )],
        Purpose::NewEnvironment(collection) => vec![Command::Store {
            collection,
            change: Change::CreateEnvironment(text),
        }],
        Purpose::RenameEnvironment(collection, id) => vec![Command::Store {
            collection,
            change: Change::RenameEnvironment { id, name: text },
        }],
        Purpose::MakeSecret(place) => {
            let Some(name) = VarName::new(&text) else {
                state.notice = Some(format!(
                    "{text:?} is not a valid name: use letters, digits, _ - and ."
                ));
                state.popup = Some(Popup::Prompt {
                    purpose: Purpose::MakeSecret(place),
                    input,
                });

                return Vec::new();
            };

            super::request::turn_into_secret(state, place, &name)
        }
    }
}

fn run_pending(state: &mut State, pending: Pending) -> Vec<Command> {
    match pending {
        Pending::Delete(key) => {
            let change = if key.path.is_empty() {
                Change::DeleteCollection
            } else {
                Change::Delete {
                    path: key.path.clone(),
                }
            };

            vec![Command::Store {
                collection: key.collection,
                change,
            }]
        }
        Pending::DeleteEnvironment(collection, id) => vec![Command::Store {
            collection,
            change: Change::DeleteEnvironment(id),
        }],
        Pending::CloseTab => super::tabs::close_now(state),
        Pending::Export(collection, path) => vec![Command::ExportBundle { collection, path }],
        Pending::Quit(_) => {
            state.should_quit = true;

            Vec::new()
        }
    }
}

/// the pop-ups that unlock or approve secrets
fn secret_popup(state: &mut State, popup: Popup, action: Action) -> Result<Vec<Command>, Action> {
    match popup {
        Popup::ChooseBackend => match action {
            Action::Choose(backend) => return Ok(vec![Command::ChooseBackend(backend)]),
            Action::ClosePopup => locked_out(state),
            other => {
                state.popup = Some(Popup::ChooseBackend);

                return Err(other);
            }
        },
        Popup::Passphrase {
            new,
            migrate,
            first,
            mut input,
            error,
        } => match action {
            Action::Type(key) => {
                input.input(key);
                state.popup = Some(Popup::Passphrase {
                    new,
                    migrate,
                    first,
                    input,
                    error,
                });
            }
            Action::ConfirmPopup => {
                let typed = Secret::new(text_of(&input));

                return Ok(super::send::passphrase(state, (new, migrate), first, typed));
            }
            Action::ClosePopup => locked_out(state),
            other => {
                state.popup = Some(Popup::Passphrase {
                    new,
                    migrate,
                    first,
                    input,
                    error,
                });

                return Err(other);
            }
        },
        Popup::Approve(approval) => match action {
            Action::ConfirmPopup => {
                let Some(id) = state.waiting.as_ref().map(|w| w.id) else {
                    return Ok(Vec::new());
                };

                return Ok(vec![Command::Approve {
                    id,
                    keys: approval.keys,
                    origin: approval.origin,
                }]);
            }
            Action::ClosePopup => {
                state.waiting = None;
                state.notice = Some("not sent".into());
            }
            other => {
                state.popup = Some(Popup::Approve(approval));

                return Err(other);
            }
        },
        _ => {}
    }

    Ok(Vec::new())
}

/// the end of a delete question that says how many secrets go too
pub(crate) fn secrets_too(count: usize) -> String {
    match count {
        0 => String::new(),
        1 => ", and 1 secret".into(),
        n => format!(", and {n} secrets"),
    }
}

fn tab_switcher(
    state: &mut State,
    mut selected: usize,
    action: Action,
) -> Result<Vec<Command>, Action> {
    let count = state.tab_count();

    match action {
        Action::Up => selected = selected.checked_sub(1).unwrap_or(count - 1),
        Action::Down => selected = (selected + 1) % count,
        Action::ConfirmPopup => super::tabs::switch_to(state, selected),
        Action::RequestAt(index) => super::tabs::switch_to(state, index),
        Action::NewTab => super::tabs::new_tab(state),
        Action::CloseTab => {
            super::tabs::switch_to(state, selected);

            let commands = super::tabs::close(state);

            // the list stays open to close another, unless closing asked
            // first or only one tab is left
            if state.popup.is_none() && state.tab_count() > 1 {
                state.popup = Some(Popup::Tabs {
                    selected: state.active_tab,
                });
            }

            return Ok(commands);
        }
        Action::ClosePopup | Action::Tabs => {}
        other => {
            state.popup = Some(Popup::Tabs { selected });

            return Err(other);
        }
    }

    if matches!(action, Action::Up | Action::Down) {
        state.popup = Some(Popup::Tabs { selected });
    }

    Ok(Vec::new())
}

fn help(
    state: &mut State,
    scroll: usize,
    here: Vec<Layer>,
    action: Action,
) -> Result<Vec<Command>, Action> {
    const PAGE: usize = 10;

    let last = super::help_lines(&state.keymap.help(&here)).saturating_sub(1);
    let scroll = match action {
        Action::Up => scroll.saturating_sub(1),
        Action::Down => scroll + 1,
        Action::PageUp => scroll.saturating_sub(PAGE),
        Action::PageDown => scroll + PAGE,
        Action::Top => 0,
        Action::Bottom => last,
        Action::ClosePopup | Action::Help => return Ok(Vec::new()),
        other => {
            state.popup = Some(Popup::Help { scroll, here });

            return Err(other);
        }
    };

    state.popup = Some(Popup::Help {
        scroll: scroll.min(last),
        here,
    });

    Ok(Vec::new())
}

/// pasted text into the field of a pop-up, if it has one
fn paste(popup: &mut Popup, text: &str) {
    let text = super::request::one_line(text);

    match popup {
        Popup::Prompt { input, .. } | Popup::Passphrase { input, .. } => {
            input.insert_str(text);
        }
        Popup::Palette(palette) => {
            palette.input.insert_str(text);
            palette.selected = 0;
        }
        Popup::Fill(fill) => {
            fill.input.insert_str(text);
        }
        Popup::Variables(editor) => {
            if let Some((_, area)) = &mut editor.table_mut().editing {
                area.insert_str(text);
            }
        }
        _ => {}
    }
}

/// the user left a secret prompt: the request waiting for it is dropped
fn locked_out(state: &mut State) {
    state.notice = Some("secrets stay locked, so nothing was done".into());
    super::send::give_back(state);
}

/// `s` in the variable editor: a plain variable becomes a secret, and a
/// secret becomes plain again, its value fetched from the store if needed
fn toggle_secret(state: &mut State, mut editor: Box<VariablesEditor>) -> Vec<Command> {
    let tab = editor.tab;
    let table = editor.table_mut();

    table.commit();

    let Some(row) = table.rows.get(table.selected).cloned() else {
        state.popup = Some(Popup::Variables(editor));

        return Vec::new();
    };

    if !row.secret {
        table.make_secret();
        state.popup = Some(Popup::Variables(editor));

        return Vec::new();
    }

    // a value typed in this editor is still at hand
    if let Some(value) = row.pending {
        table.make_plain(value.expose().to_owned());
        state.popup = Some(Popup::Variables(editor));

        return Vec::new();
    }

    let key = row
        .loaded_name
        .as_deref()
        .and_then(VarName::new)
        .and_then(|name| editor.key(tab, name));

    if let Some(key) = key {
        return super::send::reveal(state, editor, key);
    }

    editor.table_mut().make_plain(String::new());
    state.popup = Some(Popup::Variables(editor));

    Vec::new()
}

fn trust(state: &mut State, collection: LocalId, action: Action) -> Result<Vec<Command>, Action> {
    let reattach = match action {
        Action::ConfirmPopup => None,
        Action::Reattach(index) => {
            let Some((id, _)) = state.orphans.get(index) else {
                state.popup = Some(Popup::Trust { collection });

                return Ok(Vec::new());
            };

            Some(id.clone())
        }
        Action::ClosePopup => {
            let name = state
                .workspace(&collection)
                .map_or_else(String::new, |w| w.collection.name.clone());

            state.notice = Some(format!(
                "{name} is not trusted: you can look around, but nothing is sent; press {} on it to decide again",
                state.key(Layer::Sidebar, "trust")
            ));

            return Ok(Vec::new());
        }
        other => {
            state.popup = Some(Popup::Trust { collection });

            return Err(other);
        }
    };

    Ok(vec![Command::Trust {
        collection,
        reattach,
    }])
}

fn environments(
    state: &mut State,
    collection: LocalId,
    mut selected: usize,
    action: Action,
) -> Result<Vec<Command>, Action> {
    let Some(workspace) = state.workspace(&collection) else {
        return Err(action);
    };

    let environments = &workspace.collection.environments;
    let chosen = selected
        .checked_sub(1)
        .and_then(|i| environments.get(i))
        .cloned();

    // a click on an environment uses it, like choosing it and Enter
    let (chosen, action) = match action {
        Action::Pick(index) => (
            index
                .checked_sub(1)
                .and_then(|i| environments.get(i))
                .cloned(),
            Action::ConfirmPopup,
        ),
        action => (chosen, action),
    };

    match action {
        Action::Up => selected = selected.saturating_sub(1),
        Action::Down => selected = (selected + 1).min(environments.len()),
        Action::ClosePopup => return Ok(Vec::new()),
        Action::ConfirmPopup => {
            let id = chosen.map(|e| e.id);

            if let Some(workspace) = state.workspace_mut(&collection) {
                workspace.active_environment.clone_from(&id);
            }

            return Ok(vec![Command::SetActiveEnvironment {
                collection,
                environment: id,
            }]);
        }
        Action::NewEnvironment => {
            state.popup = Some(Popup::prompt(Purpose::NewEnvironment(collection), ""));

            return Ok(Vec::new());
        }
        Action::RenameEnvironment => {
            if let Some(environment) = chosen {
                state.popup = Some(Popup::prompt(
                    Purpose::RenameEnvironment(collection, environment.id),
                    &environment.name,
                ));

                return Ok(Vec::new());
            }
        }
        Action::DeleteEnvironment => {
            if let Some(environment) = chosen {
                state.popup = Some(Popup::confirm(
                    format!(
                        "Delete the environment {} and its variables{}?",
                        environment.name,
                        secrets_too(super::secret_count(workspace, Some(&environment.id)))
                    ),
                    Pending::DeleteEnvironment(collection, environment.id),
                ));

                return Ok(Vec::new());
            }
        }
        Action::EditVariables => {
            let editor = VariablesEditor::new(
                collection,
                chosen,
                &workspace.collection.variables,
                &state.globals,
            );

            state.popup = Some(Popup::Variables(Box::new(editor)));

            return Ok(Vec::new());
        }
        other => {
            state.popup = Some(Popup::Environments {
                collection,
                selected,
            });

            return Err(other);
        }
    }

    state.popup = Some(Popup::Environments {
        collection,
        selected,
    });

    Ok(Vec::new())
}

fn variables(
    state: &mut State,
    mut editor: Box<VariablesEditor>,
    action: Action,
) -> Result<Vec<Command>, Action> {
    match action {
        Action::Type(key) => editor.table_mut().input(key),
        Action::StopEdit => editor.table_mut().commit(),
        Action::NextCell => editor.table_mut().next_cell(),
        Action::NextTab | Action::PreviousTab => {
            let tabs = editor.tabs();

            editor.table_mut().commit();
            editor.tab = step(&tabs, editor.tab, matches!(action, Action::NextTab));
        }
        Action::ShowTab(index) => {
            if let Some(tab) = editor.tabs().get(index).copied() {
                editor.table_mut().commit();
                editor.tab = tab;
            }
        }
        Action::Pick(index) => {
            let table = editor.table_mut();

            table.commit();
            table.selected = index.min(table.rows.len().saturating_sub(1));
        }
        Action::EditCell(column) => {
            let table = editor.table_mut();

            if !table.rows.is_empty() {
                table.edit(column);
            }
        }
        Action::Up => editor.table_mut().select_previous(),
        Action::Down => editor.table_mut().select_next(),
        Action::AddRow => editor.table_mut().add_row(),
        Action::DeleteRow => editor.table_mut().delete_selected(),
        Action::ToggleRow => editor.table_mut().toggle_selected(),
        Action::MakeSecret => return Ok(toggle_secret(state, editor)),
        Action::Edit => {
            let table = editor.table_mut();

            if table.rows.is_empty() {
                table.add_row();
            } else {
                table.edit(Column::Name);
            }
        }
        Action::Save => return Ok(save_variables(state, editor)),
        Action::ClosePopup => {
            editor.table_mut().commit();

            if editor.is_changed() {
                state.popup = Some(Popup::SaveVariables {
                    editor,
                    quit: false,
                });
            }

            return Ok(Vec::new());
        }
        other => {
            state.popup = Some(Popup::Variables(editor));

            return Err(other);
        }
    }

    state.popup = Some(Popup::Variables(editor));

    Ok(Vec::new())
}

/// the answer to whether to save the variables: save, don't save, or go
/// back to them; when quitting, curst quits once they are saved
fn save_question(
    state: &mut State,
    editor: Box<VariablesEditor>,
    quit: bool,
    action: Action,
) -> Result<Vec<Command>, Action> {
    match action {
        Action::ConfirmPopup => {
            let commands = save_variables(state, editor);

            // a name that cannot be saved brings the editor back, to fix it
            if matches!(state.popup, Some(Popup::Variables(_))) || !quit {
                return Ok(commands);
            }

            // secrets are stored first, and the files written after them
            if state.waiting.is_some() {
                state.quit_when_saved = true;
            } else {
                super::quit(state);
            }

            Ok(commands)
        }
        Action::Discard => {
            if quit {
                super::quit(state);
            }

            Ok(Vec::new())
        }
        Action::ClosePopup => {
            state.popup = Some(Popup::Variables(editor));

            Ok(Vec::new())
        }
        other => {
            state.popup = Some(Popup::SaveVariables { editor, quit });

            Err(other)
        }
    }
}

fn save_variables(state: &mut State, mut editor: Box<VariablesEditor>) -> Vec<Command> {
    for table in &mut editor.tables {
        table.commit();
    }

    let mut commands = Vec::new();
    let mut changes = Vec::new();

    for tab in editor.tabs() {
        if !editor.changed(tab) {
            continue;
        }

        let variables = match editor.variables(tab) {
            Ok(variables) => variables,
            Err((row, message)) => {
                editor.tab = tab;
                editor.table_mut().selected = row;
                state.notice = Some(message);
                state.popup = Some(Popup::Variables(editor));

                return Vec::new();
            }
        };
        let collection = editor.collection.clone();

        changes.extend(editor.secret_changes(tab));
        commands.push(match tab {
            VarTab::Environment => {
                let Some(mut environment) = editor.environment.clone() else {
                    continue;
                };

                environment.variables = variables;

                Command::Store {
                    collection,
                    change: Change::SaveEnvironment(environment),
                }
            }
            VarTab::Collection => Command::Store {
                collection,
                change: Change::SaveVariables(variables),
            },
            VarTab::Globals => Command::SaveGlobals(variables),
        });
    }

    super::send::change_secrets(state, changes, commands)
}
