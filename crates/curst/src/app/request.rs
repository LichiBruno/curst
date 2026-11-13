//! editing the request: the URL, the request tabs and the response pane's
//! scrolling

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

        self.method = request.method;
        self.url = super::editor::single_line(&request.url, super::URL_PLACEHOLDER);
        self.params = Table::from_pairs(&request.params);
        self.headers = Table::from_pairs(&request.headers);
        self.body = BodyEditor::from_body(&request.body);
        self.auth = AuthEditor::from_auth(&request.auth);

        self.collection = Some(key.collection.clone());

        // the baseline is what the editor makes of the file, so a request
        // that was only opened never counts as changed
        self.open = Some(OpenRequest {
            key: key.clone(),
            name,
            baseline: self.request(),
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
            AuthKind::None => (false, false, false),
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

/// a variable name made from a header or parameter name
fn suggest(name: &str) -> String {
    let name: String = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let name = name.trim_matches('_');

    if name.is_empty() {
        "secret".to_owned()
    } else {
        name.to_owned()
    }
}

/// asks for a name for the flagged value at the selected place
pub(super) fn make_secret(state: &mut State) {
    let Some((place, hint)) = flagged(state) else {
        state.notice = Some("only a value marked ⚠ can be made a secret here".into());

        return;
    };

    state.popup = Some(Popup::prompt(Purpose::MakeSecret(place), &suggest(&hint)));
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
    let (scheme, secret) = ["Bearer ", "Basic ", "bearer ", "basic "]
        .iter()
        .find_map(|scheme| value.strip_prefix(scheme).map(|rest| (*scheme, rest)))
        .filter(|_| matches!(place, Place::Header(_)))
        .unwrap_or(("", value.as_str()));
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
