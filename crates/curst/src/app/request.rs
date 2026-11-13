//! editing the request: the URL, the request tabs and the response pane's
//! scrolling

use curst_core::model::{Item, Request};
use ratatui::crossterm::event::KeyEvent;

use super::editor::{AuthEditor, BodyEditor, Column, Table, show_cursor, text_of};
use super::workspace::NodeKey;
use super::{Action, AuthKind, BodyField, BodyKind, Focus, OpenRequest, PAN, RequestTab, State};

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
