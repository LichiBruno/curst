mod editor;
mod keys;
mod popup;
mod request;
mod send;
mod sidebar;
mod tabs;
mod view_model;
mod workspace;

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;

use curst_core::http::{HttpError, Response};
use curst_core::model::{EnvId, Environment, LocalId, Method, Request, Variable};
use curst_core::secrets::{Approval, Secret, SecretKey, SecretScope};
use curst_core::store::Backend;
use ratatui::crossterm::event::KeyEvent;
use ratatui_textarea::TextArea;

pub(crate) use editor::{AuthEditor, AuthKind, BodyEditor, BodyField, BodyKind, Column, Table};
pub(crate) use keys::action_for;
pub(crate) use popup::{Pending, Popup, Purpose, VarTab};
pub(crate) use view_model::{ResponseView, display, skip_columns};
pub(crate) use workspace::{NodeKey, RowKind, Sidebar, Workspace, rows};

#[cfg(test)]
pub(crate) use workspace::tests::sample;

use editor::single_line;

const URL_PLACEHOLDER: &str = "https://example.com/api, then Enter to send";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Focus {
    Sidebar,
    Method,
    #[default]
    Url,
    Request,
    Response,
}

impl Focus {
    const ORDER: [Self; 5] = [
        Self::Sidebar,
        Self::Method,
        Self::Url,
        Self::Request,
        Self::Response,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum RequestTab {
    #[default]
    Params,
    Headers,
    Body,
    Auth,
}

impl RequestTab {
    pub(crate) const ALL: [Self; 4] = [Self::Params, Self::Headers, Self::Body, Self::Auth];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Params => "Params",
            Self::Headers => "Headers",
            Self::Body => "Body",
            Self::Auth => "Auth",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ResponseTab {
    #[default]
    Body,
    Headers,
    Timing,
    Raw,
}

impl ResponseTab {
    pub(crate) const ALL: [Self; 4] = [Self::Body, Self::Headers, Self::Timing, Self::Raw];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Body => "Body",
            Self::Headers => "Headers",
            Self::Timing => "Timing",
            Self::Raw => "Raw",
        }
    }
}

fn step<T: Copy + PartialEq>(all: &[T], current: T, forward: bool) -> T {
    let index = all.iter().position(|t| *t == current).unwrap_or(0);
    let next = if forward {
        (index + 1) % all.len()
    } else {
        (index + all.len() - 1) % all.len()
    };

    all.get(next).copied().unwrap_or(current)
}

#[derive(Debug)]
pub(crate) enum Outcome {
    Response(Box<ResponseView>),
    Error(String),
}

/// what the secret store needs before it can be used
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Locked {
    /// a choice between the keychain and the encrypted file
    Backend,

    /// the passphrase of the encrypted file; `new` when the file does not
    /// exist yet, so the passphrase is asked twice
    Passphrase { new: bool, error: Option<String> },

    /// the store cannot be opened
    Failed(String),
}

/// work held back until the secret store is open or an origin approved
#[derive(Debug)]
pub(crate) struct Waiting {
    pub(crate) id: u64,
    pub(crate) then: Then,
}

#[derive(Debug)]
pub(crate) enum Then {
    /// a request, as typed, its templates unresolved
    Send(Request),

    /// secret changes, and the file writes that follow them once they
    /// succeed
    Save {
        changes: Vec<SecretChange>,
        files: Vec<Command>,
    },

    /// moving every secret to another backend
    Migrate {
        to: Backend,
        passphrase: Option<Secret>,
    },

    /// a secret turning back into a plain variable: the editor waits, out
    /// of sight, for the value to come from the store
    Reveal {
        editor: Box<popup::VariablesEditor>,
        key: SecretKey,
    },
}

/// one change to the secret store
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SecretChange {
    Set(SecretKey, Secret),
    Rename(SecretKey, SecretKey),
    Delete(SecretKey),
    DeleteScope(SecretScope),
}

/// the saved request the editor holds
#[derive(Debug)]
pub(crate) struct OpenRequest {
    pub(crate) key: NodeKey,
    pub(crate) name: String,

    /// the editor's request right after loading or saving
    baseline: Request,
}

#[derive(Debug)]
pub(crate) struct State {
    pub(crate) should_quit: bool,
    pub(crate) focus: Focus,
    pub(crate) method: Method,
    pub(crate) url: TextArea<'static>,
    pub(crate) request_tab: RequestTab,
    pub(crate) params: Table,
    pub(crate) headers: Table,
    pub(crate) body: BodyEditor,
    pub(crate) auth: AuthEditor,
    pub(crate) response_tab: ResponseTab,
    pub(crate) outcome: Option<Outcome>,
    pub(crate) scroll: usize,

    /// the first character shown of every response line
    pub(crate) column: usize,

    /// rows the response pane showed in the last frame, recorded by the view
    /// so scrolling stops at the last full page
    pub(crate) response_height: Cell<usize>,

    /// columns the response pane showed in the last frame, recorded by the
    /// view so sideways scrolling stops at the longest line
    pub(crate) response_width: Cell<usize>,
    pub(crate) in_flight: Option<u64>,
    pub(crate) notice: Option<String>,
    pub(crate) collections: Vec<Workspace>,
    pub(crate) globals: Vec<Variable>,
    pub(crate) sidebar: Sidebar,
    pub(crate) open: Option<OpenRequest>,
    pub(crate) popup: Option<Popup>,

    /// collections whose folder has gone, offered when a new folder is
    /// trusted, so it can take over their secrets
    pub(crate) orphans: Vec<(LocalId, PathBuf)>,

    /// secret values fetched this session; `None` for a secret with no
    /// value in the store
    pub(crate) secret_values: HashMap<SecretKey, Option<Secret>>,
    pub(crate) waiting: Option<Waiting>,

    /// the last request sent, as typed, for following a stopped redirect
    pub(crate) last_sent: Option<Request>,

    /// where secrets are kept, `None` until the user picks
    pub(crate) backend: Option<Backend>,

    /// the collection the active tab belongs to: the one its saved request
    /// is in; `None` for a request outside any collection, which only global
    /// variables reach
    pub(crate) collection: Option<LocalId>,

    /// the tabs that are not active, in order, and where the active one sits
    /// among them
    pub(crate) others: Vec<tabs::Parked>,
    pub(crate) active_tab: usize,
    next_id: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            should_quit: false,
            focus: Focus::default(),
            method: Method::default(),
            url: single_line("", URL_PLACEHOLDER),
            request_tab: RequestTab::default(),
            params: Table::default(),
            headers: Table::default(),
            body: BodyEditor::default(),
            auth: AuthEditor::default(),
            response_tab: ResponseTab::default(),
            outcome: None,
            scroll: 0,
            column: 0,
            response_height: Cell::new(PAGE),
            response_width: Cell::new(0),
            in_flight: None,
            notice: None,
            collections: Vec::new(),
            globals: Vec::new(),
            sidebar: Sidebar::default(),
            open: None,
            popup: None,
            orphans: Vec::new(),
            secret_values: HashMap::new(),
            waiting: None,
            last_sent: None,
            backend: None,
            collection: None,
            others: Vec::new(),
            active_tab: 0,
            next_id: 0,
        }
    }
}

impl State {
    pub(crate) fn workspace(&self, id: &LocalId) -> Option<&Workspace> {
        self.collections.iter().find(|w| w.id == *id)
    }

    fn workspace_mut(&mut self, id: &LocalId) -> Option<&mut Workspace> {
        self.collections.iter_mut().find(|w| w.id == *id)
    }

    /// the collection whose variables and environment apply: the active
    /// tab's, never just the one selected in the sidebar
    pub(crate) fn current_workspace(&self) -> Option<&Workspace> {
        self.workspace(self.collection.as_ref()?)
    }

    /// the collection the environment picker works on: the active tab's,
    /// otherwise the one selected in the sidebar
    fn picked_workspace(&self) -> Option<&Workspace> {
        self.current_workspace().or_else(|| {
            let selected = self.sidebar.selected.as_ref()?;

            self.workspace(&selected.collection)
        })
    }

    fn response_line_count(&self) -> usize {
        match &self.outcome {
            Some(Outcome::Response(view)) => view.lines(self.response_tab).len(),
            Some(Outcome::Error(_)) | None => 0,
        }
    }

    /// how far the response can scroll sideways before the longest line of
    /// the tab ends
    pub(crate) fn last_column(&self) -> usize {
        match &self.outcome {
            Some(Outcome::Response(view)) => view
                .width(self.response_tab)
                .saturating_sub(self.response_width.get()),
            Some(Outcome::Error(_)) | None => 0,
        }
    }

    fn reset_scroll(&mut self) {
        self.scroll = 0;
        self.column = 0;
    }
}

#[derive(Debug)]
pub(crate) enum Action {
    Quit,
    Send,
    Cancel,
    Save,
    FocusNext,
    FocusPrevious,
    NextTab,
    PreviousTab,
    NextMethod,
    PreviousMethod,
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Left,
    Right,
    Edit,
    EditContentType,
    StopEdit,
    NextCell,
    AddRow,
    DeleteRow,
    ToggleRow,
    CycleKind,
    Type(KeyEvent),
    OpenSave,
    NewRequest,
    NewFolder,
    NewCollection,
    Rename,
    DeleteItem,
    Cut,
    Paste,
    MoveUp,
    MoveDown,
    Trust,

    /// trust a new folder and give it the secrets of orphan number n
    Reattach(usize),
    Follow,
    Choose(Backend),
    Environments,
    EditVariables,
    NewEnvironment,
    RenameEnvironment,
    DeleteEnvironment,
    ClosePopup,
    ConfirmPopup,
    Responded {
        id: u64,
        result: Box<Result<ResponseView, HttpError>>,
    },
    Saved(Result<PathBuf, String>),
    Loaded {
        collections: Vec<Workspace>,
        globals: Vec<Variable>,
        problems: Vec<String>,
        orphans: Vec<(LocalId, PathBuf)>,
    },
    Trusted {
        previous: LocalId,
        result: Result<Box<Workspace>, String>,
    },
    SecretsLoaded {
        id: u64,
        values: Vec<(SecretKey, Option<Secret>)>,
    },
    SecretsLocked {
        id: Option<u64>,
        locked: Locked,
    },
    Unlocked,
    BackendChosen(Result<Backend, String>),
    NeedsApproval {
        id: u64,
        approval: Box<Approval>,
    },
    Approved {
        id: u64,
        result: Result<(), String>,
    },
    SecretsChanged {
        id: u64,
        result: Result<(), String>,
    },
    MakeSecret,
    NewTab,
    CloseTab,
    LeaveCollection,
    NextRequest,
    PreviousRequest,

    /// the tab at this position; past the end, the last one
    RequestAt(usize),

    /// opens the tab switcher
    Tabs,
    Settings,
    Migrate,
    Migrated {
        id: u64,
        result: Result<(Backend, usize), String>,
    },
    BackendKnown(Option<Backend>),
    EditBody,
    BodyEdited(Result<String, String>),
    Stored {
        collection: LocalId,
        result: Result<Box<Stored>, String>,
    },
    CollectionAdded(Result<Box<Workspace>, String>),
    GlobalsSaved(Result<Vec<Variable>, String>),
    Notice(String),
}

impl Action {
    /// the action for a finished request; preparing the view is the slow
    /// part of a large response, so the worker thread calls this
    pub(crate) fn responded(id: u64, result: Result<Response, HttpError>) -> Self {
        Self::Responded {
            id,
            result: Box::new(result.map(ResponseView::new)),
        }
    }
}

/// a collection after a change was written, reloaded from disk
#[derive(Debug, Default)]
pub(crate) struct Stored {
    /// the collection, `None` once it is deleted
    pub(crate) workspace: Option<Workspace>,

    /// an item that was renamed or moved, from where to where
    pub(crate) moved: Option<(NodeKey, NodeKey)>,

    /// a new item, to select in the sidebar
    pub(crate) created: Option<NodeKey>,

    /// where the editor's request was just saved, and what was written;
    /// keys typed since then still count as unsaved
    pub(crate) saved: Option<(NodeKey, Request)>,

    /// files that could not be read
    pub(crate) problems: Vec<String>,
}

/// a change to one collection, run by the runtime
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Change {
    CreateRequest {
        parent: Vec<String>,
        name: String,
        request: Request,

        /// whether the editor now holds this request
        open: bool,
    },
    SaveRequest {
        path: Vec<String>,
        request: Request,
    },
    CreateFolder {
        parent: Vec<String>,
        name: String,
    },
    Rename {
        path: Vec<String>,
        name: String,
    },
    Move {
        path: Vec<String>,
        parent: Vec<String>,
    },
    Reorder {
        path: Vec<String>,
        index: usize,
    },
    Delete {
        path: Vec<String>,
    },
    RenameCollection(String),
    DeleteCollection,
    SaveVariables(Vec<Variable>),
    CreateEnvironment(String),
    SaveEnvironment(Environment),
    RenameEnvironment {
        id: EnvId,
        name: String,
    },
    DeleteEnvironment(EnvId),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Send {
        id: u64,
        request: Request,

        /// the secrets the request carries; the runtime checks they are
        /// approved for its origin, and keeps redirects within approved ones
        secrets: Vec<SecretKey>,
    },
    LoadSecrets {
        id: u64,
        keys: Vec<SecretKey>,
    },
    Unlock(Secret),
    ChooseBackend(Backend),
    Approve {
        id: u64,
        keys: Vec<SecretKey>,
        origin: String,
    },
    ChangeSecrets {
        id: u64,
        changes: Vec<SecretChange>,
    },
    Migrate {
        id: u64,
        to: Backend,
        passphrase: Option<Secret>,
    },

    /// opens the body in the user's editor; the runtime does it, since the
    /// terminal has to be handed over
    EditBody {
        text: String,
        extension: &'static str,
    },
    Cancel,
    SaveBody(PathBuf),
    Store {
        collection: LocalId,
        change: Change,
    },
    CreateCollection(String),
    SaveGlobals(Vec<Variable>),
    Trust {
        collection: LocalId,
        reattach: Option<LocalId>,
    },
    SetActiveEnvironment {
        collection: LocalId,
        environment: Option<EnvId>,
    },
}

const PAGE: usize = 10;

/// columns moved by one sideways scroll
const PAN: usize = 8;

const NO_COLLECTION: &str = "create a collection first: press c in the sidebar";

pub(crate) fn update(state: &mut State, action: Action) -> Vec<Command> {
    let background = reported(&action);

    if !background {
        state.notice = None;
    }

    let commands = match popup::apply(state, action) {
        Ok(commands) => commands,
        Err(action) => apply(state, action),
    };

    state.sync_cursors();

    commands
}

fn apply(state: &mut State, action: Action) -> Vec<Command> {
    if reported(&action) {
        return report(state, action);
    }

    match action {
        Action::Quit => quit(state),
        Action::Send => return send::send(state),
        Action::Follow => return send::follow(state),
        Action::MakeSecret => request::make_secret(state),
        Action::NewTab => tabs::new_tab(state),
        Action::CloseTab => return tabs::close(state),
        Action::LeaveCollection => tabs::leave_collection(state),
        Action::NextRequest | Action::PreviousRequest => {
            tabs::step(state, matches!(action, Action::NextRequest));
        }
        Action::RequestAt(index) => tabs::switch_to(state, index),
        Action::Tabs => {
            state.popup = Some(Popup::Tabs {
                selected: state.active_tab,
            });
        }
        Action::Settings => state.popup = Some(Popup::Settings),
        Action::EditBody => return request::edit_body(state),
        Action::Save => return save(state),
        Action::Cancel => return cancel(state),
        Action::FocusNext | Action::FocusPrevious => {
            state.stop_editing();
            state.focus = step(
                &Focus::ORDER,
                state.focus,
                matches!(action, Action::FocusNext),
            );
        }
        Action::NextTab | Action::PreviousTab => {
            let forward = matches!(action, Action::NextTab);

            match state.focus {
                Focus::Request => {
                    state.request_tab = step(&RequestTab::ALL, state.request_tab, forward);
                }
                Focus::Response => {
                    state.response_tab = step(&ResponseTab::ALL, state.response_tab, forward);
                    state.reset_scroll();
                }
                Focus::Sidebar | Focus::Method | Focus::Url => {}
            }
        }
        Action::NextMethod => state.method = state.method.next(),
        Action::PreviousMethod => state.method = state.method.previous(),
        Action::Up
        | Action::Down
        | Action::Top
        | Action::Bottom
        | Action::Left
        | Action::Right
        | Action::Edit
            if state.focus == Focus::Sidebar =>
        {
            return sidebar::apply(state, &action);
        }
        Action::Up
        | Action::Down
        | Action::PageUp
        | Action::PageDown
        | Action::Top
        | Action::Bottom
        | Action::Left
        | Action::Right => request::navigate(state, &action),
        Action::Edit => request::edit(state),
        Action::EditContentType
        | Action::StopEdit
        | Action::NextCell
        | Action::AddRow
        | Action::DeleteRow
        | Action::ToggleRow
        | Action::CycleKind => request::change(state, &action),
        Action::Type(key) => request::type_key(state, key),
        Action::OpenSave => open_save(state),
        Action::NewRequest
        | Action::NewFolder
        | Action::NewCollection
        | Action::Rename
        | Action::DeleteItem
        | Action::Cut
        | Action::Paste
        | Action::MoveUp
        | Action::MoveDown
        | Action::Trust => return sidebar::apply(state, &action),
        Action::Environments => open_environments(state),
        // reports went to `report`, and these only mean something in a pop-up
        _ => {}
    }

    Vec::new()
}

/// applies what the runtime reports back: responses, saved files and
/// store changes
fn report(state: &mut State, action: Action) -> Vec<Command> {
    match action {
        Action::SecretsLoaded { id, values } => return send::loaded(state, id, values),
        Action::Unlocked => return send::unlocked(state),
        Action::BackendChosen(result) => return send::backend_chosen(state, result),
        Action::Approved { id, result } => return send::approved(state, id, result),
        Action::SecretsChanged { id, result } => return send::changed(state, id, result),
        Action::Migrated { id, result } => send::migrated(state, id, result),
        Action::BackendKnown(backend) => state.backend = backend,
        Action::BodyEdited(result) => match result {
            Ok(text) => state.body.text = editor::body_text(&text),
            Err(message) => state.notice = Some(message),
        },
        Action::SecretsLocked { id, locked } => send::locked(state, id, locked),
        Action::NeedsApproval { id, approval } => send::needs_approval(state, id, *approval),
        Action::Responded { id, result } if state.in_flight != Some(id) => {
            let outcome = match *result {
                Ok(view) => Outcome::Response(Box::new(view)),
                Err(err) => Outcome::Error(err.to_string()),
            };

            tabs::deliver(state, id, outcome);
        }
        Action::Responded { id, result } => {
            if state.in_flight == Some(id) {
                if let Ok(view) = result.as_ref()
                    && let Some(stopped) = &view.response.stopped
                {
                    state.notice = Some(format!(
                        "the redirect to {} was not followed, since it would carry secrets there; press f in the response to follow it",
                        display(&stopped.url)
                    ));
                }

                state.in_flight = None;
                state.reset_scroll();
                state.outcome = Some(match *result {
                    Ok(view) => Outcome::Response(Box::new(view)),
                    Err(err) => Outcome::Error(err.to_string()),
                });
            }
        }
        Action::Saved(result) => saved_body(state, result),
        Action::Loaded {
            collections,
            globals,
            problems,
            orphans,
        } => {
            state.orphans = orphans;
            loaded(state, collections, globals, &problems);
        }
        Action::Trusted { previous, result } => match result {
            Ok(workspace) => trusted(state, &previous, *workspace),
            Err(message) => state.notice = Some(message),
        },
        Action::Stored { collection, result } => match result {
            Ok(stored) => return stored_change(state, &collection, *stored),
            Err(message) => state.notice = Some(message),
        },
        Action::CollectionAdded(result) => match result {
            Ok(workspace) => {
                let key = NodeKey::root(workspace.id.clone());

                state.collections.push(*workspace);
                state.sidebar.expanded.insert(key.clone());
                state.sidebar.selected = Some(key);
            }
            Err(message) => state.notice = Some(message),
        },
        Action::GlobalsSaved(result) => match result {
            Ok(globals) => state.globals = globals,
            Err(message) => state.notice = Some(message),
        },
        Action::Notice(message) => state.notice = Some(message),
        _ => {}
    }

    Vec::new()
}

/// stops a request waiting for its secrets, or one in flight
/// whether `action` is something the runtime reports back, rather than a
/// key the user pressed
fn reported(action: &Action) -> bool {
    matches!(
        action,
        Action::Responded { .. }
            | Action::Loaded { .. }
            | Action::Trusted { .. }
            | Action::SecretsLoaded { .. }
            | Action::SecretsLocked { .. }
            | Action::Unlocked
            | Action::BackendChosen(_)
            | Action::NeedsApproval { .. }
            | Action::Approved { .. }
            | Action::SecretsChanged { .. }
            | Action::Migrated { .. }
            | Action::BackendKnown(_)
            | Action::BodyEdited(_)
            | Action::Stored { .. }
            | Action::CollectionAdded(_)
            | Action::GlobalsSaved(_)
            | Action::Saved(_)
            | Action::Notice(_)
    )
}

fn cancel(state: &mut State) -> Vec<Command> {
    if state.waiting.is_some() {
        send::give_back(state);
        state.notice = Some("cancelled".into());
    } else if state.in_flight.take().is_some() {
        state.notice = Some("request cancelled".into());

        return vec![Command::Cancel];
    }

    Vec::new()
}

fn quit(state: &mut State) {
    let asked = matches!(
        state.popup,
        Some(Popup::Confirm {
            pending: Pending::Quit(_),
            ..
        })
    );
    let variables = matches!(&state.popup, Some(Popup::Variables(editor)) if editor.is_changed());

    if (state.any_dirty() || variables) && !asked {
        // on no, the pop-up that was open comes back
        let previous = state.popup.take().map(Box::new);

        state.popup = Some(Popup::confirm(
            "Quit without saving your changes?",
            Pending::Quit(previous),
        ));
    } else {
        state.should_quit = true;
    }
}

fn save(state: &mut State) -> Vec<Command> {
    state.stop_editing();

    if let Some(open) = &state.open {
        return vec![Command::Store {
            collection: open.key.collection.clone(),
            change: Change::SaveRequest {
                path: open.key.path.clone(),
                request: state.request(),
            },
        }];
    }

    match sidebar::target_folder(state) {
        Some(parent) => {
            let name = suggested_name(state);

            state.popup = Some(Popup::prompt(Purpose::SaveAs(parent), &name));
        }
        None => state.notice = Some(NO_COLLECTION.into()),
    }

    Vec::new()
}

/// a starting name for a new request: the last plain part of its URL path
fn suggested_name(state: &State) -> String {
    let url = state.request().url;
    let path = url.split(['?', '#']).next().unwrap_or_default();

    path.rsplit('/')
        .find(|part| !part.is_empty() && !part.contains("{{") && !part.contains(':'))
        .unwrap_or_default()
        .to_owned()
}

fn open_environments(state: &mut State) {
    let Some(workspace) = state.picked_workspace() else {
        state.notice = Some(NO_COLLECTION.into());

        return;
    };

    let selected = workspace.active_environment.as_ref().map_or(0, |id| {
        workspace
            .collection
            .environments
            .iter()
            .position(|e| e.id == *id)
            .map_or(0, |i| i + 1)
    });

    state.popup = Some(Popup::Environments {
        collection: workspace.id.clone(),
        selected,
    });
}

fn saved_body(state: &mut State, result: Result<PathBuf, String>) {
    match result {
        Ok(path) => {
            state.popup = None;
            state.notice = Some(format!("saved to {}", path.display()));
        }
        Err(message) => state.notice = Some(message),
    }
}

fn loaded(
    state: &mut State,
    collections: Vec<Workspace>,
    globals: Vec<Variable>,
    problems: &[String],
) {
    for workspace in &collections {
        state
            .sidebar
            .expanded
            .insert(NodeKey::root(workspace.id.clone()));
    }

    if state.sidebar.selected.is_none() {
        let first = collections
            .iter()
            .find(|w| w.trusted)
            .or_else(|| collections.first());

        state.sidebar.selected = first.map(|w| NodeKey::root(w.id.clone()));
    }

    let untrusted = collections
        .iter()
        .find(|w| !w.trusted)
        .map(|w| w.id.clone());

    state.collections = collections;
    state.globals = globals;

    if let Some(collection) = untrusted {
        state.popup = Some(Popup::Trust { collection });
    }

    if let Some(first) = problems.first() {
        state.notice = Some(match problems.len() {
            1 => first.clone(),
            n => format!("{first} (and {} more)", n - 1),
        });
    }
}

/// a collection now trusted, perhaps under the id of the collection whose
/// secrets it took over
fn trusted(state: &mut State, previous: &LocalId, workspace: Workspace) {
    let from = NodeKey::root(previous.clone());
    let to = NodeKey::root(workspace.id.clone());

    state.notice = Some(format!("{} is trusted", workspace.collection.name));
    state.orphans.retain(|(id, _)| *id != workspace.id);
    state
        .collections
        .retain(|w| w.id != *previous && w.id != workspace.id);
    state.collections.push(workspace);
    state.sidebar.rebase(&from, &to);

    if let Some(open) = &mut state.open {
        open.key = open.key.rebased(&from, &to);
    }

    if state.collection.as_ref() == Some(previous) {
        state.collection = Some(to.collection.clone());
    }

    tabs::rebase(state, &from, &to);
}

/// the secrets that go with what a change removed: a whole collection, or
/// environments; asked of the store only when they had secret variables
fn removed_secrets(before: Option<&Workspace>, after: Option<&Workspace>) -> Vec<SecretChange> {
    let Some(before) = before else {
        return Vec::new();
    };

    let has_secrets = |variables: &[Variable]| variables.iter().any(|v| v.secret);

    match after {
        None => {
            let any = has_secrets(&before.collection.variables)
                || before
                    .collection
                    .environments
                    .iter()
                    .any(|e| has_secrets(&e.variables));

            if any {
                vec![SecretChange::DeleteScope(SecretScope::Collection(
                    before.id.clone(),
                ))]
            } else {
                Vec::new()
            }
        }
        Some(after) => before
            .collection
            .environments
            .iter()
            .filter(|e| after.collection.environment(&e.id).is_none() && has_secrets(&e.variables))
            .map(|e| {
                SecretChange::DeleteScope(SecretScope::Environment(before.id.clone(), e.id.clone()))
            })
            .collect(),
    }
}

/// how many secret variables a collection, or one of its environments,
/// holds, for a delete confirmation
pub(crate) fn secret_count(workspace: &Workspace, environment: Option<&EnvId>) -> usize {
    let count = |variables: &[Variable]| variables.iter().filter(|v| v.secret).count();

    match environment {
        Some(id) => workspace
            .collection
            .environment(id)
            .map_or(0, |e| count(&e.variables)),
        None => {
            count(&workspace.collection.variables)
                + workspace
                    .collection
                    .environments
                    .iter()
                    .map(|e| count(&e.variables))
                    .sum::<usize>()
        }
    }
}

fn stored_change(state: &mut State, collection: &LocalId, stored: Stored) -> Vec<Command> {
    let Stored {
        workspace,
        moved,
        created,
        saved,
        problems,
    } = stored;
    let doomed = removed_secrets(state.workspace(collection), workspace.as_ref());

    if let Some(workspace) = workspace {
        match state.workspace_mut(collection) {
            Some(slot) => *slot = workspace,
            None => state.collections.push(workspace),
        }
    } else {
        state.collections.retain(|w| w.id != *collection);
        forget(state, &NodeKey::root(collection.clone()));
    }

    if let Some((from, to)) = moved {
        state.sidebar.rebase(&from, &to);

        if let Some(open) = &mut state.open {
            open.key = open.key.rebased(&from, &to);
        }

        tabs::rebase(state, &from, &to);
    }

    forget_missing(state);

    if let Some(key) = created {
        if let Some(parent) = key.parent() {
            state.sidebar.expanded.insert(parent);
        }

        state.sidebar.selected = Some(key);
    }

    if let Some((key, written)) = saved {
        state.notice = Some("saved".into());
        state.collection = Some(key.collection.clone());
        state.open = Some(OpenRequest {
            key,
            name: String::new(),
            baseline: written,
        });
    }

    // a reload never touches the editor, only the name shown for it
    let name = state.open.as_ref().and_then(|open| {
        state
            .workspace(&open.key.collection)
            .and_then(|w| w.collection.item(&open.key.path))
            .map(|item| item.name().to_owned())
    });

    if let (Some(open), Some(name)) = (&mut state.open, name) {
        open.name = name;
    }

    if let Some(first) = problems.first() {
        state.notice = Some(first.clone());
    }

    send::change_secrets(state, doomed, Vec::new())
}

/// drops keys inside `gone`; an open request stays in the editor, unsaved
fn forget(state: &mut State, gone: &NodeKey) {
    if state.open.as_ref().is_some_and(|o| o.key.is_within(gone)) {
        state.open = None;
    }

    if gone.path.is_empty() && state.collection.as_ref() == Some(&gone.collection) {
        state.collection = None;
    }

    tabs::forget(state, gone);

    if state
        .sidebar
        .cut
        .as_ref()
        .is_some_and(|k| k.is_within(gone))
    {
        state.sidebar.cut = None;
    }

    if state
        .sidebar
        .selected
        .as_ref()
        .is_some_and(|k| k.is_within(gone))
    {
        state.sidebar.selected = gone
            .parent()
            .filter(|p| state.workspace(&p.collection).is_some())
            .or_else(|| {
                state
                    .collections
                    .first()
                    .map(|w| NodeKey::root(w.id.clone()))
            });
    }
}

/// after a reload, forgets keys whose item no longer exists
fn forget_missing(state: &mut State) {
    let exists = |state: &State, key: &NodeKey| {
        state
            .workspace(&key.collection)
            .is_some_and(|w| key.path.is_empty() || w.collection.item(&key.path).is_some())
    };
    let mut kept: Vec<NodeKey> = [
        state.open.as_ref().map(|o| o.key.clone()),
        state.sidebar.selected.clone(),
        state.sidebar.cut.clone(),
    ]
    .into_iter()
    .flatten()
    .collect();

    kept.extend(tabs::open_keys(state));

    for key in kept {
        if !exists(state, &key) {
            forget(state, &key);
        }
    }

    let expanded: Vec<NodeKey> = state.sidebar.expanded.iter().cloned().collect();

    for key in expanded {
        if !exists(state, &key) {
            state.sidebar.expanded.remove(&key);
        }
    }
}

fn open_save(state: &mut State) {
    let Some(Outcome::Response(view)) = &state.outcome else {
        state.notice = Some("nothing to save yet".into());

        return;
    };

    let name = format!("response.{}", view.extension());

    state.popup = Some(Popup::prompt(Purpose::SaveBody, &name));
}

#[cfg(test)]
pub(crate) mod tests;
