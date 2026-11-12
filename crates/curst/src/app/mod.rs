mod editor;
mod keys;
mod view_model;

use std::cell::Cell;
use std::path::PathBuf;

use curst_core::http::{HttpError, Response};
use curst_core::model::{Method, Request};
use ratatui::crossterm::event::KeyEvent;
use ratatui_textarea::TextArea;

pub(crate) use editor::{AuthEditor, AuthKind, BodyEditor, BodyField, BodyKind, Column, Table};
pub(crate) use keys::action_for;
pub(crate) use view_model::{ResponseView, skip_columns};

use editor::{show_cursor, single_line, text_of};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Focus {
    Method,
    #[default]
    Url,
    Request,
    Response,
}

impl Focus {
    fn next(self) -> Self {
        match self {
            Self::Method => Self::Url,
            Self::Url => Self::Request,
            Self::Request => Self::Response,
            Self::Response => Self::Method,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Method => Self::Response,
            Self::Url => Self::Method,
            Self::Request => Self::Url,
            Self::Response => Self::Request,
        }
    }
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
    pub(crate) save_prompt: Option<TextArea<'static>>,
    pub(crate) notice: Option<String>,
    next_id: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            should_quit: false,
            focus: Focus::default(),
            method: Method::default(),
            url: single_line("", "https://example.com/api, then Enter to send"),
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
            save_prompt: None,
            notice: None,
            next_id: 0,
        }
    }
}

impl State {
    /// the table shown by the request pane, if the current tab has one
    pub(crate) fn active_table(&mut self) -> Option<&mut Table> {
        match self.request_tab {
            RequestTab::Params => Some(&mut self.params),
            RequestTab::Headers => Some(&mut self.headers),
            RequestTab::Body if self.body.kind == BodyKind::Form => Some(&mut self.body.form),
            RequestTab::Body | RequestTab::Auth => None,
        }
    }

    /// whether keys go to a text field in the request pane
    pub(crate) fn request_typing(&self) -> bool {
        match self.request_tab {
            RequestTab::Params => self.params.editing.is_some(),
            RequestTab::Headers => self.headers.editing.is_some(),
            RequestTab::Body if self.body.kind == BodyKind::Form => {
                self.body.form.editing.is_some()
            }
            RequestTab::Body => self.body.typing.is_some(),
            RequestTab::Auth => self.auth.typing,
        }
    }

    pub(crate) fn request(&self) -> Request {
        Request {
            method: self.method,
            url: text_of(&self.url).trim().to_owned(),
            params: self.params.pairs(),
            headers: self.headers.pairs(),
            body: self.body.body(),
            auth: self.auth.auth(),
        }
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

    fn stop_editing(&mut self) {
        self.params.commit();
        self.headers.commit();
        self.body.form.commit();
        self.body.typing = None;
        self.auth.typing = false;
    }

    fn sync_cursors(&mut self) {
        let request = self.focus == Focus::Request;

        show_cursor(&mut self.url, self.focus == Focus::Url);

        let body = request && self.request_tab == RequestTab::Body;

        show_cursor(
            &mut self.body.text,
            body && self.body.typing == Some(BodyField::Text),
        );
        show_cursor(
            &mut self.body.content_type,
            body && self.body.typing == Some(BodyField::ContentType),
        );

        let typing = request && self.request_tab == RequestTab::Auth && self.auth.typing;
        let selected = self.auth.selected;
        let (username, password, token) = match self.auth.kind {
            AuthKind::None => (false, false, false),
            AuthKind::Basic => (selected == 0, selected == 1, false),
            AuthKind::Bearer => (false, false, selected == 0),
        };

        show_cursor(&mut self.auth.username, typing && username);
        show_cursor(&mut self.auth.password, typing && password);
        show_cursor(&mut self.auth.token, typing && token);
    }
}

#[derive(Debug)]
pub(crate) enum Action {
    Quit,
    Send,
    Cancel,
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
    ConfirmSave,
    Responded {
        id: u64,
        result: Box<Result<ResponseView, HttpError>>,
    },
    Saved(Result<PathBuf, String>),
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

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Send { id: u64, request: Request },
    Cancel,
    SaveBody(PathBuf),
}

const PAGE: usize = 10;

/// columns moved by one sideways scroll
const PAN: usize = 8;

pub(crate) fn update(state: &mut State, action: Action) -> Vec<Command> {
    if !matches!(action, Action::Responded { .. }) {
        state.notice = None;
    }

    let commands = apply(state, action);

    state.sync_cursors();

    commands
}

fn apply(state: &mut State, action: Action) -> Vec<Command> {
    match action {
        Action::Quit => state.should_quit = true,
        Action::Send => return send(state),
        Action::Cancel => {
            if state.in_flight.take().is_some() {
                state.notice = Some("request cancelled".into());

                return vec![Command::Cancel];
            }
        }
        Action::FocusNext | Action::FocusPrevious => {
            state.stop_editing();

            state.focus = if matches!(action, Action::FocusNext) {
                state.focus.next()
            } else {
                state.focus.previous()
            };
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
                Focus::Method | Focus::Url => {}
            }
        }
        Action::NextMethod => state.method = state.method.next(),
        Action::PreviousMethod => state.method = state.method.previous(),
        Action::Up
        | Action::Down
        | Action::PageUp
        | Action::PageDown
        | Action::Top
        | Action::Bottom
        | Action::Left
        | Action::Right => navigate(state, &action),
        Action::Edit => edit(state),
        Action::EditContentType => {
            if state.request_tab == RequestTab::Body && state.body.kind == BodyKind::Text {
                state.body.typing = Some(BodyField::ContentType);
            }
        }
        Action::StopEdit => {
            if state.save_prompt.take().is_none() {
                state.stop_editing();
            }
        }
        Action::NextCell => {
            if state.body.typing == Some(BodyField::ContentType) {
                state.body.typing = Some(BodyField::Text);
            } else if state.request_tab == RequestTab::Auth {
                state.auth.select_next();
            } else if let Some(table) = state.active_table() {
                table.next_cell();
            }
        }
        Action::AddRow | Action::DeleteRow | Action::ToggleRow => change_row(state, &action),
        Action::CycleKind => match state.request_tab {
            RequestTab::Body => state.body.kind = state.body.kind.next(),
            RequestTab::Auth => state.auth.cycle_kind(),
            RequestTab::Params | RequestTab::Headers => {}
        },
        Action::Type(key) => type_key(state, key),
        Action::OpenSave => open_save(state),
        Action::ConfirmSave => {
            if let Some(prompt) = &state.save_prompt {
                let path = text_of(prompt).trim().to_owned();

                if !path.is_empty() {
                    return vec![Command::SaveBody(PathBuf::from(path))];
                }
            }
        }
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
        Action::Saved(result) => match result {
            Ok(path) => {
                state.save_prompt = None;
                state.notice = Some(format!("saved to {}", path.display()));
            }
            Err(message) => state.notice = Some(message),
        },
    }

    Vec::new()
}

fn change_row(state: &mut State, action: &Action) {
    let Some(table) = state.active_table() else {
        return;
    };

    match action {
        Action::AddRow => table.add_row(),
        Action::DeleteRow => table.delete_selected(),
        _ => table.toggle_selected(),
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

    let mut commands = Vec::new();

    if state.in_flight.is_some() {
        commands.push(Command::Cancel);
    }

    state.next_id += 1;
    state.in_flight = Some(state.next_id);

    commands.push(Command::Send {
        id: state.next_id,
        request,
    });

    commands
}

fn navigate(state: &mut State, action: &Action) {
    match state.focus {
        Focus::Response => {
            let page = state.response_height.get().max(1);
            let last = state.response_line_count().saturating_sub(page);
            let last_column = state.last_column();

            match action {
                Action::Up => state.scroll = state.scroll.saturating_sub(1).min(last),
                Action::Down => state.scroll = (state.scroll + 1).min(last),
                Action::PageUp => state.scroll = state.scroll.saturating_sub(page).min(last),
                Action::PageDown => state.scroll = (state.scroll + page).min(last),
                Action::Top => state.scroll = 0,
                Action::Bottom => state.scroll = last,
                Action::Left => {
                    state.column = state.column.saturating_sub(PAN).min(last_column);
                }
                Action::Right => state.column = (state.column + PAN).min(last_column),
                _ => {}
            }
        }
        Focus::Request if state.request_tab == RequestTab::Auth => match action {
            Action::Up => state.auth.select_previous(),
            Action::Down => state.auth.select_next(),
            _ => {}
        },
        Focus::Request => {
            if let Some(table) = state.active_table() {
                match action {
                    Action::Up => table.select_previous(),
                    Action::Down => table.select_next(),
                    _ => {}
                }
            }
        }
        Focus::Method | Focus::Url => {}
    }
}

fn edit(state: &mut State) {
    match state.request_tab {
        RequestTab::Body if state.body.kind == BodyKind::None => {
            state.notice = Some("choose a body type first, press t".into());
        }
        RequestTab::Body if state.body.kind != BodyKind::Form => {
            state.body.typing = Some(BodyField::Text);
        }
        RequestTab::Auth => state.auth.typing = state.auth.kind != AuthKind::None,
        _ => {
            if let Some(table) = state.active_table() {
                if table.rows.is_empty() {
                    table.add_row();
                } else {
                    table.edit(Column::Name);
                }
            }
        }
    }
}

fn type_key(state: &mut State, key: KeyEvent) {
    if let Some(prompt) = &mut state.save_prompt {
        prompt.input(key);

        return;
    }

    match state.focus {
        Focus::Url => {
            state.url.input(key);
        }
        Focus::Request => match state.request_tab {
            RequestTab::Body if state.body.kind != BodyKind::Form => {
                if let Some(field) = state.body.field() {
                    field.input(key);
                }
            }
            RequestTab::Auth => {
                if let Some(field) = state.auth.selected_field() {
                    field.input(key);
                }
            }
            _ => {
                if let Some(table) = state.active_table() {
                    table.input(key);
                }
            }
        },
        Focus::Method | Focus::Response => {}
    }
}

fn open_save(state: &mut State) {
    let Some(Outcome::Response(view)) = &state.outcome else {
        state.notice = Some("nothing to save yet".into());

        return;
    };

    let name = format!("response.{}", view.extension());

    state.save_prompt = Some(single_line(&name, "file name"));
}

#[cfg(test)]
pub(crate) mod tests;
