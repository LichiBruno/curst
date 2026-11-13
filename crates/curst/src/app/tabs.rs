//! several requests open at once, each in a tab with its own editor,
//! response and collection
//!
//! the active tab's fields live in [`State`] itself, so everything that
//! edits a request works on them directly; the other tabs wait in
//! `State::others`, and switching swaps the two

use std::mem;

use curst_core::model::{LocalId, Method, Request};
use ratatui_textarea::TextArea;

use super::editor::{AuthEditor, BodyEditor, Table, single_line};
use super::popup::{Pending, Popup};
use super::workspace::NodeKey;
use super::{
    Action, Command, OpenRequest, Outcome, RequestTab, ResponseTab, State, URL_PLACEHOLDER,
};

/// a request open in a tab that is not the active one
#[derive(Debug)]
pub(crate) struct Parked {
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
    pub(crate) column: usize,
    pub(crate) in_flight: Option<u64>,
    pub(crate) open: Option<OpenRequest>,
    pub(crate) last_sent: Option<Request>,
    pub(crate) collection: Option<LocalId>,
}

impl Default for Parked {
    fn default() -> Self {
        Self {
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
            in_flight: None,
            open: None,
            last_sent: None,
            collection: None,
        }
    }
}

impl Parked {
    fn is_dirty(&self) -> bool {
        let request = Request {
            method: self.method,
            url: super::editor::text_of(&self.url).trim().to_owned(),
            params: self.params.pairs(),
            headers: self.headers.pairs(),
            body: self.body.body(),
            auth: self.auth.auth(),
        };

        self.open
            .as_ref()
            .is_some_and(|open| request != open.baseline)
    }
}

/// what the tab bar shows for one tab
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Label {
    pub(crate) method: Method,
    pub(crate) name: String,
    pub(crate) dirty: bool,
    pub(crate) active: bool,
}

/// the active tab's fields, taken out of `state` and replaced by a blank tab
fn stash(state: &mut State) -> Parked {
    let blank = Parked::default();

    Parked {
        method: mem::replace(&mut state.method, blank.method),
        url: mem::replace(&mut state.url, blank.url),
        request_tab: mem::replace(&mut state.request_tab, blank.request_tab),
        params: mem::replace(&mut state.params, blank.params),
        headers: mem::replace(&mut state.headers, blank.headers),
        body: mem::replace(&mut state.body, blank.body),
        auth: mem::replace(&mut state.auth, blank.auth),
        response_tab: mem::replace(&mut state.response_tab, blank.response_tab),
        outcome: state.outcome.take(),
        scroll: mem::take(&mut state.scroll),
        column: mem::take(&mut state.column),
        in_flight: state.in_flight.take(),
        open: state.open.take(),
        last_sent: state.last_sent.take(),
        collection: state.collection.take(),
    }
}

/// makes `tab` the active one
fn unstash(state: &mut State, tab: Parked) {
    state.method = tab.method;
    state.url = tab.url;
    state.request_tab = tab.request_tab;
    state.params = tab.params;
    state.headers = tab.headers;
    state.body = tab.body;
    state.auth = tab.auth;
    state.response_tab = tab.response_tab;
    state.outcome = tab.outcome;
    state.scroll = tab.scroll;
    state.column = tab.column;
    state.in_flight = tab.in_flight;
    state.open = tab.open;
    state.last_sent = tab.last_sent;
    state.collection = tab.collection;
}

impl State {
    /// how many tabs are open
    pub(crate) fn tab_count(&self) -> usize {
        self.others.len() + 1
    }

    /// whether any tab, the active one or another, has unsaved changes
    pub(crate) fn any_dirty(&self) -> bool {
        self.is_dirty() || self.others.iter().any(Parked::is_dirty)
    }

    /// whether the active tab holds nothing worth keeping: never saved,
    /// never sent and empty
    pub(super) fn is_blank(&self) -> bool {
        self.open.is_none()
            && self.outcome.is_none()
            && self.in_flight.is_none()
            && self.request() == Request::default()
    }

    /// every tab, in order, for the tab bar
    pub(crate) fn tab_labels(&self) -> Vec<Label> {
        let label = |method, open: Option<&OpenRequest>, url: String, dirty, active| Label {
            method,
            name: open.map_or_else(|| short_url(&url), |o| o.name.clone()),
            dirty,
            active,
        };
        let mut labels: Vec<Label> = self
            .others
            .iter()
            .map(|tab| {
                label(
                    tab.method,
                    tab.open.as_ref(),
                    super::editor::text_of(&tab.url),
                    tab.is_dirty(),
                    false,
                )
            })
            .collect();
        let active = label(
            self.method,
            self.open.as_ref(),
            self.request().url,
            self.is_dirty(),
            true,
        );

        labels.insert(self.active_tab.min(labels.len()), active);

        labels
    }
}

/// a short name for a request that was never saved
fn short_url(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let rest = rest.split(['?', '#']).next().unwrap_or_default();

    if rest.is_empty() {
        "new request".to_owned()
    } else {
        rest.to_owned()
    }
}

/// what the tab keys do
pub(crate) fn apply(state: &mut State, action: &Action) -> Vec<Command> {
    match action {
        Action::NewTab => new_tab(state),
        Action::CloseTab => return close(state),
        Action::LeaveCollection => leave_collection(state),
        Action::NextRequest | Action::PreviousRequest => {
            step(state, matches!(action, Action::NextRequest));
        }
        Action::RequestAt(index) => switch_to(state, *index),
        _ => {}
    }

    Vec::new()
}

/// makes tab `index` the active one
pub(crate) fn switch_to(state: &mut State, index: usize) {
    let last = state.tab_count() - 1;
    let index = index.min(last);

    if index == state.active_tab {
        return;
    }

    let current = stash(state);

    state.others.insert(state.active_tab, current);

    let next = state.others.remove(index);

    unstash(state, next);
    state.active_tab = index;
    state.stop_editing();
}

/// the next or previous tab, wrapping around
pub(crate) fn step(state: &mut State, forward: bool) {
    let count = state.tab_count();
    let index = if forward {
        (state.active_tab + 1) % count
    } else {
        (state.active_tab + count - 1) % count
    };

    switch_to(state, index);
}

/// opens a blank tab after the active one, outside any collection
pub(crate) fn new_tab(state: &mut State) {
    let current = stash(state);

    state.others.insert(state.active_tab, current);
    state.active_tab += 1;
    state.notice = Some("new request, outside any collection".into());
}

/// closes the active tab, asking first when it has unsaved changes
pub(crate) fn close(state: &mut State) -> Vec<Command> {
    if state.is_dirty() {
        let name = state
            .open
            .as_ref()
            .map_or_else(String::new, |o| o.name.clone());

        state.popup = Some(Popup::confirm(
            format!("Close {name} without saving your changes?"),
            Pending::CloseTab,
        ));

        return Vec::new();
    }

    close_now(state)
}

/// closes the active tab; the last one is replaced by a blank tab
pub(crate) fn close_now(state: &mut State) -> Vec<Command> {
    let cancel = state.in_flight.is_some();
    let closed = stash(state);

    drop(closed);

    if !state.others.is_empty() {
        let index = state.active_tab.min(state.others.len() - 1);
        let next = state.others.remove(index);

        unstash(state, next);
        state.active_tab = index;
    }

    // a request still running in the closed tab is stopped
    if cancel {
        vec![Command::Cancel]
    } else {
        Vec::new()
    }
}

/// detaches the active tab from its collection: what it holds stays, as a
/// request that was never saved, and only global variables apply to it
pub(crate) fn leave_collection(state: &mut State) {
    if state.collection.is_none() && state.open.is_none() {
        state.notice = Some("this request is not in a collection".into());

        return;
    }

    state.open = None;
    state.collection = None;
    state.notice = Some("left the collection: only global variables apply now".into());
}

/// opens the saved request at `key`: in its own tab if it is open already,
/// otherwise in the active tab when that is blank, or in a new one
pub(crate) fn open_request(state: &mut State, key: &NodeKey) {
    if state.open.as_ref().is_some_and(|o| o.key == *key) {
        return;
    }

    let existing = state
        .others
        .iter()
        .position(|tab| tab.open.as_ref().is_some_and(|o| o.key == *key));

    if let Some(position) = existing {
        let index = if position >= state.active_tab {
            position + 1
        } else {
            position
        };

        switch_to(state, index);

        return;
    }

    if !state.is_blank() {
        new_tab(state);
    }

    state.load_request(key);
}

/// gives a response to the tab that sent it, when that is not the active
/// one; returns false when no tab is waiting for `id`
pub(crate) fn deliver(state: &mut State, id: u64, outcome: Outcome) -> bool {
    let Some(tab) = state.others.iter_mut().find(|t| t.in_flight == Some(id)) else {
        return false;
    };

    tab.in_flight = None;
    tab.scroll = 0;
    tab.column = 0;
    tab.outcome = Some(outcome);

    true
}

/// makes active the tab that sent `id`, if another tab did
pub(crate) fn focus_sender(state: &mut State, id: u64) {
    let position = state.others.iter().position(|t| t.in_flight == Some(id));

    if let Some(position) = position {
        let index = if position >= state.active_tab {
            position + 1
        } else {
            position
        };

        switch_to(state, index);
    }
}

/// follows a rename or move into every other tab
pub(crate) fn rebase(state: &mut State, from: &NodeKey, to: &NodeKey) {
    for tab in &mut state.others {
        if let Some(open) = &mut tab.open {
            open.key = open.key.rebased(from, to);
        }

        if tab.collection.as_ref() == Some(&from.collection) && from.path.is_empty() {
            tab.collection = Some(to.collection.clone());
        }
    }
}

/// drops what other tabs held of something that is gone; what they show
/// stays, as requests that were never saved
pub(crate) fn forget(state: &mut State, gone: &NodeKey) {
    for tab in &mut state.others {
        if tab.open.as_ref().is_some_and(|o| o.key.is_within(gone)) {
            tab.open = None;
        }

        if gone.path.is_empty() && tab.collection.as_ref() == Some(&gone.collection) {
            tab.collection = None;
        }
    }
}

/// the saved requests the other tabs hold, to check after a reload
pub(crate) fn open_keys(state: &State) -> Vec<NodeKey> {
    state
        .others
        .iter()
        .filter_map(|t| t.open.as_ref().map(|o| o.key.clone()))
        .collect()
}

impl State {
    /// the saved requests open with unsaved edits, in any tab
    pub(crate) fn unsaved_keys(&self) -> Vec<NodeKey> {
        let mut keys: Vec<NodeKey> = self
            .others
            .iter()
            .filter(|tab| tab.is_dirty())
            .filter_map(|tab| tab.open.as_ref().map(|o| o.key.clone()))
            .collect();

        if self.is_dirty()
            && let Some(open) = &self.open
        {
            keys.push(open.key.clone());
        }

        keys
    }
}

/// shows the request saved at `key` again where a tab holds it unchanged,
/// after it was replaced on disk; a tab with edits keeps them
pub(crate) fn reload_if_unchanged(state: &mut State, key: &NodeKey) {
    if state.open.as_ref().is_some_and(|o| o.key == *key) && !state.is_dirty() {
        state.load_request(key);
    }

    let saved = state
        .workspace(&key.collection)
        .and_then(|w| w.collection.item(&key.path))
        .and_then(|item| match item {
            curst_core::model::Item::Request(saved) => Some(saved.request.clone()),
            curst_core::model::Item::Folder(_) => None,
        });
    let Some(request) = saved else {
        return;
    };

    for tab in &mut state.others {
        let holds = tab.open.as_ref().is_some_and(|o| o.key == *key);

        if !holds || tab.is_dirty() {
            continue;
        }

        tab.method = request.method;
        tab.url = single_line(&request.url, URL_PLACEHOLDER);
        tab.params = Table::from_pairs(&request.params);
        tab.headers = Table::from_pairs(&request.headers);
        tab.body = BodyEditor::from_body(&request.body);
        tab.auth = AuthEditor::from_auth(&request.auth);

        if let Some(open) = &mut tab.open {
            open.baseline.clone_from(&request);
        }
    }
}
