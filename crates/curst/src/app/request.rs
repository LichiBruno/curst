//! editing the request: the URL, the request tabs and the response pane's
//! scrolling

use curst_core::curl;
use curst_core::model::{Auth, Item, Request, Variable};
use curst_core::secrets::detect::{self, Place};
use curst_core::secrets::{Secret, SecretKey};
use curst_core::vars::VarName;
use ratatui::crossterm::event::KeyEvent;

use super::editor::{AuthEditor, BodyEditor, Column, Table, show_cursor, text_of};
use super::popup::{Popup, Purpose};
use super::workspace::NodeKey;
use super::{
    Action, AuthKind, BodyField, BodyKind, Change, Command, Focus, Layer, OpenRequest, PAN,
    RequestTab, SecretChange, State,
};

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

    /// the request as the editor shows it, with its templates unresolved
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

    /// whether the open request has changes that are not saved; a request
    /// that was never saved has nothing to lose
    pub(crate) fn is_dirty(&self) -> bool {
        self.open
            .as_ref()
            .is_some_and(|open| self.request() != open.baseline)
    }

    /// fills the editors with `request`
    pub(crate) fn show(&mut self, request: &Request) {
        self.method = request.method;
        self.url = super::editor::single_line(&request.url, super::URL_PLACEHOLDER);
        self.params = Table::from_pairs(&request.params);
        self.headers = Table::from_pairs(&request.headers);
        self.body = BodyEditor::from_body(&request.body);
        self.auth = AuthEditor::from_auth(&request.auth);
    }

    /// fills the editor from the saved request at `key`
    pub(crate) fn load_request(&mut self, key: &NodeKey) {
        let Some(workspace) = self.workspace(&key.collection) else {
            return;
        };

        let Some(Item::Request(saved)) = workspace.collection.item(&key.path) else {
            return;
        };

        let request = saved.request.clone();
        let name = saved.name.clone();

        self.show(&request);
        self.collection = Some(key.collection.clone());

        // the baseline is what the editor makes of the file, so a request
        // that was only opened never counts as changed
        self.open = Some(OpenRequest {
            key: key.clone(),
            name,
            baseline: self.request(),
            tried: None,
        });
    }

    pub(super) fn stop_editing(&mut self) {
        self.params.commit();
        self.headers.commit();
        self.body.form.commit();
        self.body.typing = None;
        self.auth.typing = false;
    }

    pub(super) fn sync_cursors(&mut self) {
        let request = self.focus == Focus::Request && self.popup.is_none();

        // the auth dialog shows a cursor in its selected field only
        if let Some(super::popup::Popup::Auth(dialog)) = &mut self.popup {
            let row = dialog.row;
            let editor = &mut dialog.editor;

            show_cursor(&mut editor.username, row == 1);
            show_cursor(&mut editor.password, row == 2);
            show_cursor(&mut editor.token, row == 1);
        }

        show_cursor(
            &mut self.url,
            self.focus == Focus::Url && self.popup.is_none(),
        );

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
            AuthKind::Inherit | AuthKind::None => (false, false, false),
            AuthKind::Basic => (selected == 0, selected == 1, false),
            AuthKind::Bearer => (false, false, selected == 0),
        };

        show_cursor(&mut self.auth.username, typing && username);
        show_cursor(&mut self.auth.password, typing && password);
        show_cursor(&mut self.auth.token, typing && token);
    }
}

/// the editing keys of the request pane that are not typing
pub(super) fn change(state: &mut State, action: &Action) {
    match action {
        Action::EditContentType => {
            if state.request_tab == RequestTab::Body && state.body.kind == BodyKind::Text {
                state.body.typing = Some(BodyField::ContentType);
            }
        }
        Action::StopEdit => state.stop_editing(),
        Action::NextCell => {
            if state.body.typing == Some(BodyField::ContentType) {
                state.body.typing = Some(BodyField::Text);
            } else if state.request_tab == RequestTab::Auth {
                state.auth.select_next();
            } else if let Some(table) = state.active_table() {
                table.next_cell();
            }
        }
        Action::CycleKind => match state.request_tab {
            RequestTab::Body => state.body.kind = state.body.kind.next(),
            RequestTab::Auth => state.auth.cycle_kind(),
            RequestTab::Params | RequestTab::Headers => {}
        },
        _ => change_row(state, action),
    }
}

fn change_row(state: &mut State, action: &Action) {
    let Some(table) = state.active_table() else {
        return;
    };

    match action {
        Action::AddRow => table.add_row(),
        Action::DeleteRow => table.delete_selected(),
        Action::ToggleRow => table.toggle_selected(),
        _ => {}
    }
}

pub(super) fn navigate(state: &mut State, action: &Action) {
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
        Focus::Sidebar | Focus::Method | Focus::Url => {}
    }
}

pub(super) fn edit(state: &mut State) {
    match state.request_tab {
        RequestTab::Body if state.body.kind == BodyKind::None => {
            state.notice = Some(format!(
                "choose a body type first, press {}",
                state.key(Layer::Request, "kind")
            ));
        }
        RequestTab::Body if state.body.kind != BodyKind::Form => {
            state.body.typing = Some(BodyField::Text);
        }
        RequestTab::Auth => state.auth.typing = state.auth.kind.field_count() > 0,
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

pub(super) fn type_key(state: &mut State, key: KeyEvent) {
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
        Focus::Sidebar | Focus::Method | Focus::Response => {}
    }
}

/// hands the body to the user's editor, as typed, templates unresolved
pub(super) fn edit_body(state: &mut State) -> Vec<Command> {
    let extension = match state.body.kind {
        BodyKind::Json => "json",
        BodyKind::Text => "txt",
        BodyKind::None | BodyKind::Form => {
            state.notice = Some(format!(
                "choose a JSON or text body first, press {}",
                state.key(Layer::Request, "kind")
            ));

            return Vec::new();
        }
    };

    state.stop_editing();

    vec![Command::EditBody {
        text: super::editor::text_of(&state.body.text),
        extension,
    }]
}

/// the flagged value at the selected place, with a name to suggest for it
fn flagged(state: &State) -> Option<(Place, String)> {
    let row = |table: &Table, header: bool| {
        let row = table.rows.get(table.selected)?;

        detect::check_pair(&row.name, &row.value, header).map(|_| row.name.clone())
    };

    match state.request_tab {
        RequestTab::Params => {
            row(&state.params, false).map(|n| (Place::Param(state.params.selected), n))
        }
        RequestTab::Headers => {
            row(&state.headers, true).map(|n| (Place::Header(state.headers.selected), n))
        }
        RequestTab::Body if state.body.kind == BodyKind::Form => {
            row(&state.body.form, false).map(|n| (Place::FormField(state.body.form.selected), n))
        }
        RequestTab::Body => None,
        RequestTab::Auth => {
            let request = Request {
                auth: state.auth.auth(),
                ..Request::default()
            };

            detect::detect(&request).first().map(|f| match f.place {
                Place::Password => (Place::Password, "password".to_owned()),
                _ => (Place::Token, "token".to_owned()),
            })
        }
    }
}

/// asks for a name for the flagged value at the selected place
pub(super) fn make_secret(state: &mut State) {
    let Some((place, hint)) = flagged(state) else {
        state.notice = Some("only a value marked ⚠ can be made a secret here".into());

        return;
    };

    state.popup = Some(Popup::prompt(
        Purpose::MakeSecret(place),
        &curl::name_for(&hint),
    ));
}

fn value_at(state: &State, place: Place) -> Option<String> {
    let cell = |table: &Table, index: usize| table.rows.get(index).map(|r| r.value.clone());

    match place {
        Place::Param(index) => cell(&state.params, index),
        Place::Header(index) => cell(&state.headers, index),
        Place::FormField(index) => cell(&state.body.form, index),
        Place::Password => match state.auth.auth() {
            Auth::Basic { password, .. } => Some(password),
            _ => None,
        },
        Place::Token => match state.auth.auth() {
            Auth::Bearer { token } => Some(token),
            _ => None,
        },
        Place::UrlUserInfo | Place::UrlQuery(_) => None,
    }
}

fn put_at(state: &mut State, place: Place, text: String) {
    let set = |table: &mut Table, index: usize| {
        if let Some(row) = table.rows.get_mut(index) {
            row.value.clone_from(&text);
        }
    };

    match place {
        Place::Param(index) => set(&mut state.params, index),
        Place::Header(index) => set(&mut state.headers, index),
        Place::FormField(index) => set(&mut state.body.form, index),
        Place::Password => {
            if let Auth::Basic { username, .. } = state.auth.auth() {
                state.auth = AuthEditor::from_auth(&Auth::Basic {
                    username,
                    password: text,
                });
            }
        }
        Place::Token => state.auth = AuthEditor::from_auth(&Auth::Bearer { token: text }),
        Place::UrlUserInfo | Place::UrlQuery(_) => {}
    }
}

fn upsert(variables: &mut Vec<Variable>, variable: Variable) {
    match variables.iter_mut().find(|v| v.name == variable.name) {
        Some(slot) => *slot = variable,
        None => variables.push(variable),
    }
}

/// stores the value at `place` as the secret `name`, in the active
/// environment, the collection, or the globals when there is no
/// collection, and puts `{{name}}` in its place
pub(super) fn turn_into_secret(state: &mut State, place: Place, name: &VarName) -> Vec<Command> {
    let Some(value) = value_at(state, place) else {
        state.notice = Some("that value is gone".into());

        return Vec::new();
    };

    let workspace = state.current_workspace();

    if workspace.is_some_and(|w| !w.trusted) {
        state.notice = Some("trust this collection before keeping secrets for it".into());

        return Vec::new();
    }

    // an auth scheme stays in the field: `Bearer abc` becomes
    // `Bearer {{token}}`, and only `abc` is the secret
    let (scheme, secret) = if matches!(place, Place::Header(_)) {
        curl::split_scheme(&value)
    } else {
        ("", value.as_str())
    };
    let secret = Secret::new(secret);
    let (key, file) = secret_home(state, Variable::secret(name.clone()));

    put_at(state, place, format!("{scheme}{{{{{name}}}}}"));
    state.notice = Some(if state.open.is_some() {
        format!(
            "{{{{{name}}}}} holds it now; the plain value stays in the saved request until you press {}",
            state.key(Layer::Global, "save")
        )
    } else {
        format!("{{{{{name}}}}} holds it now")
    });

    super::send::change_secrets(state, vec![SecretChange::Set(key, secret)], vec![file])
}

/// where a new secret variable goes: the active environment, otherwise the
/// collection, otherwise the globals; with the file write that records it
fn secret_home(state: &State, variable: Variable) -> (SecretKey, Command) {
    let name = variable.name.clone();
    let Some(workspace) = state.current_workspace() else {
        let mut globals = state.globals.clone();

        upsert(&mut globals, variable);

        return (SecretKey::Global { name }, Command::SaveGlobals(globals));
    };

    let environment = workspace
        .active_environment
        .as_ref()
        .and_then(|id| workspace.collection.environment(id));

    if let Some(environment) = environment {
        let mut environment = environment.clone();

        upsert(&mut environment.variables, variable);

        return (
            SecretKey::Environment {
                collection: workspace.id.clone(),
                environment: environment.id.clone(),
                name,
            },
            Command::Store {
                collection: workspace.id.clone(),
                change: Change::SaveEnvironment(environment),
            },
        );
    }

    let mut variables = workspace.collection.variables.clone();

    upsert(&mut variables, variable);

    (
        SecretKey::Collection {
            collection: workspace.id.clone(),
            name,
        },
        Command::Store {
            collection: workspace.id.clone(),
            change: Change::SaveVariables(variables),
        },
    )
}

/// puts the cursor where a field was clicked, giving it the focus first
pub(super) fn place(state: &mut State, field: super::Field, row: u16, col: u16) {
    match field {
        super::Field::Url => {
            if state.focus != Focus::Url {
                state.stop_editing();
                state.focus = Focus::Url;
            }

            super::mouse::place(&mut state.url, row, col);
        }
        super::Field::Body => {
            if state.focus != Focus::Request {
                state.stop_editing();
                state.focus = Focus::Request;
            }

            if !matches!(state.body.kind, BodyKind::Json | BodyKind::Text) {
                return;
            }

            state.body.typing = Some(BodyField::Text);
            super::mouse::place(&mut state.body.text, row, col);
        }
        super::Field::Popup => {}
    }
}

/// scrolls the response by `lines`, up when negative, wherever the focus is
pub(super) fn scroll_response(state: &mut State, lines: isize) {
    let page = state.response_height.get().max(1);
    let last = state.response_line_count().saturating_sub(page);

    state.scroll = state.scroll.saturating_add_signed(lines).min(last);
}

impl State {
    /// the table the request pane shows, if its tab has one
    fn shown_table(&self) -> Option<&Table> {
        match self.request_tab {
            RequestTab::Params => Some(&self.params),
            RequestTab::Headers => Some(&self.headers),
            RequestTab::Body if self.body.kind == BodyKind::Form => Some(&self.body.form),
            RequestTab::Body | RequestTab::Auth => None,
        }
    }

    /// the selected row of the request pane's table, when it shows one
    pub(crate) fn selected_row(&self) -> Option<usize> {
        let table = self.shown_table()?;

        (!table.rows.is_empty()).then_some(table.selected)
    }

    /// the cell of the request pane's table being edited
    pub(crate) fn editing_column(&self) -> Option<Column> {
        self.shown_table()?
            .editing
            .as_ref()
            .map(|(column, _)| *column)
    }
}

/// text pasted into a one-line field: line breaks and other control
/// characters become spaces
pub(super) fn one_line(text: &str) -> String {
    text.trim_matches(['\r', '\n'])
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// text pasted where the focus is: a curl command becomes a request, and
/// anything else goes into the field being typed in
pub(super) fn paste(state: &mut State, text: &str) -> Vec<Command> {
    let typing_body = state.focus == Focus::Request
        && state.request_tab == RequestTab::Body
        && state.body.typing == Some(BodyField::Text);
    let typing = state.focus == Focus::Url || state.request_typing();

    if !typing_body && is_curl(text) {
        return import_curl(state, text);
    }

    if typing_body {
        let text: String = text
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .collect();

        state.body.text.insert_str(text);

        return Vec::new();
    }

    if !typing {
        return Vec::new();
    }

    let text = one_line(text);

    match state.focus {
        Focus::Url => {
            state.url.insert_str(text);
        }
        Focus::Request => {
            if let Some(field) = state.body.field() {
                field.insert_str(text);
            } else if state.request_tab == RequestTab::Auth {
                if let Some(field) = state.auth.selected_field() {
                    field.insert_str(text);
                }
            } else if let Some(table) = state.active_table()
                && let Some((_, area)) = &mut table.editing
            {
                area.insert_str(text);
            }
        }
        Focus::Sidebar | Focus::Method | Focus::Response => {}
    }

    Vec::new()
}

/// whether pasted text is a curl command
pub(crate) fn is_curl(text: &str) -> bool {
    let text = text.trim_start();

    text.strip_prefix("curl")
        .is_some_and(|rest| rest.starts_with(char::is_whitespace))
}

/// a pasted curl command as a request, in a tab of its own outside any
/// collection; what looks like a credential is kept as a global secret
/// and the request holds `{{name}}` in its place
fn import_curl(state: &mut State, text: &str) -> Vec<Command> {
    if state.waiting.is_some() {
        state.notice = Some("not imported: secrets are still being stored, try again".into());

        return Vec::new();
    }

    let globals = state.globals.clone();
    let taken = |name: &str| globals.iter().any(|v| v.name.as_str() == name);
    let imported = match curl::import(text, taken) {
        Ok(imported) => imported,
        Err(err) => {
            state.notice = Some(format!("not imported: {err}"));

            return Vec::new();
        }
    };

    if !state.is_blank() || state.collection.is_some() {
        super::tabs::new_tab(state);
    }

    state.show(&imported.request);
    state.focus = Focus::Url;

    let names: Vec<String> = imported
        .credentials
        .iter()
        .map(|c| format!("{{{{{}}}}}", c.name))
        .collect();
    let mut notice = match names.len() {
        0 => "imported from curl".to_owned(),
        1 => format!(
            "imported from curl; {} holds a credential as a secret",
            names[0]
        ),
        _ => format!(
            "imported from curl; {} hold credentials as secrets",
            names.join(", ")
        ),
    };

    if let Some(first) = imported.warnings.first() {
        let more = imported.warnings.len() - 1;
        let more = if more > 0 {
            format!(" (and {more} more)")
        } else {
            String::new()
        };

        notice = format!("{notice}; left out {first}{more}");
    }

    state.notice = Some(notice);

    if imported.credentials.is_empty() {
        return Vec::new();
    }

    let mut globals = state.globals.clone();
    let mut changes = Vec::new();

    for credential in imported.credentials {
        upsert(&mut globals, Variable::secret(credential.name.clone()));
        changes.push(SecretChange::Set(
            SecretKey::Global {
                name: credential.name,
            },
            credential.value,
        ));
    }

    super::send::change_secrets(state, changes, vec![Command::SaveGlobals(globals)])
}
