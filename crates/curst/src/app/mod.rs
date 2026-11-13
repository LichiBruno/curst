mod editor;
mod keys;
mod popup;
mod request;
mod sidebar;
mod view_model;
mod workspace;

use std::cell::Cell;
use std::fmt::Write;
use std::path::PathBuf;

use curst_core::http::{HttpError, Response};
use curst_core::model::{EnvId, Environment, LocalId, Method, Request, Variable};
use curst_core::vars::{Resolution, Scopes, resolve_request};
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

    /// the collection whose variables and environment apply: the open
    /// request's, otherwise the one selected in the sidebar
    pub(crate) fn current_workspace(&self) -> Option<&Workspace> {
        let key = self
            .open
            .as_ref()
            .map(|o| &o.key)
            .or(self.sidebar.selected.as_ref())?;

        self.workspace(&key.collection)
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
    },
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
    },
    Cancel,
    SaveBody(PathBuf),
    Store {
        collection: LocalId,
        change: Change,
    },
    CreateCollection(String),
    SaveGlobals(Vec<Variable>),
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
    let background = matches!(
        action,
        Action::Responded { .. }
            | Action::Loaded { .. }
            | Action::Stored { .. }
            | Action::CollectionAdded(_)
            | Action::GlobalsSaved(_)
    );

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
    match action {
        Action::Quit => quit(state),
        Action::Send => return send(state),
        Action::Save => return save(state),
        Action::Cancel => {
            if state.in_flight.take().is_some() {
                state.notice = Some("request cancelled".into());

                return vec![Command::Cancel];
            }
        }
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
        | Action::MoveDown => return sidebar::apply(state, &action),
        Action::Environments => open_environments(state),
        Action::Responded { .. }
        | Action::Saved(_)
        | Action::Loaded { .. }
        | Action::Stored { .. }
        | Action::CollectionAdded(_)
        | Action::GlobalsSaved(_)
        | Action::Notice(_) => report(state, action),
        Action::EditVariables
        | Action::NewEnvironment
        | Action::RenameEnvironment
        | Action::DeleteEnvironment
        | Action::ClosePopup
        | Action::ConfirmPopup => {}
    }

    Vec::new()
}

/// applies what the runtime reports back: responses, saved files and
/// store changes
fn report(state: &mut State, action: Action) {
    match action {
        Action::Responded { id, result } => {
            if state.in_flight == Some(id) {
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
        } => loaded(state, collections, globals, &problems),
        Action::Stored { collection, result } => match result {
            Ok(stored) => stored_change(state, &collection, *stored),
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

    if (state.is_dirty() || variables) && !asked {
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

fn send(state: &mut State) -> Vec<Command> {
    state.stop_editing();

    let request = state.request();

    if request.url.is_empty() {
        state.notice = Some("type a URL first".into());
        state.focus = Focus::Url;

        return Vec::new();
    }

    let workspace = state.current_workspace();
    let scopes = Scopes {
        environment: workspace.map_or(&[], Workspace::environment_variables),
        collection: workspace.map_or(&[], |w| w.collection.variables.as_slice()),
        global: &state.globals,
    };
    let (resolved, resolution) = resolve_request(&request, &scopes);

    if !resolution.is_complete() {
        state.notice = Some(not_sent(workspace, &resolution));

        return Vec::new();
    }

    let mut commands = Vec::new();

    if state.in_flight.is_some() {
        commands.push(Command::Cancel);
    }

    state.next_id += 1;
    state.in_flight = Some(state.next_id);

    commands.push(Command::Send {
        id: state.next_id,
        request: resolved,
    });

    commands
}

/// why a request was not sent, naming every variable that blocks it
fn not_sent(workspace: Option<&Workspace>, resolution: &Resolution) -> String {
    let mut message = String::from("not sent:");

    if !resolution.missing.is_empty() {
        let names: Vec<String> = resolution
            .missing
            .iter()
            .map(|n| format!("{{{{{n}}}}}"))
            .collect();
        let place = workspace
            .and_then(Workspace::environment_name)
            .map_or_else(String::new, |env| format!(" in {env}"));

        let _ = write!(message, " {} not defined{place}", names.join(", "));
    }

    if !resolution.cyclic.is_empty() {
        let names: Vec<String> = resolution.cyclic.iter().map(ToString::to_string).collect();

        let _ = write!(message, " {} refer to themselves", names.join(", "));
    }

    message
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
    let Some(workspace) = state.current_workspace() else {
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
        state.sidebar.selected = collections.first().map(|w| NodeKey::root(w.id.clone()));
    }

    state.collections = collections;
    state.globals = globals;

    if let Some(first) = problems.first() {
        state.notice = Some(match problems.len() {
            1 => format!("skipped {first}"),
            n => format!("skipped {first} and {} more", n - 1),
        });
    }
}

fn stored_change(state: &mut State, collection: &LocalId, stored: Stored) {
    let Stored {
        workspace,
        moved,
        created,
        saved,
        problems,
    } = stored;

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
        state.notice = Some(format!("skipped {first}"));
    }
}

/// drops keys inside `gone`; an open request stays in the editor, unsaved
fn forget(state: &mut State, gone: &NodeKey) {
    if state.open.as_ref().is_some_and(|o| o.key.is_within(gone)) {
        state.open = None;
    }

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
    let kept = [
        state.open.as_ref().map(|o| o.key.clone()),
        state.sidebar.selected.clone(),
        state.sidebar.cut.clone(),
    ];

    for key in kept.into_iter().flatten() {
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
