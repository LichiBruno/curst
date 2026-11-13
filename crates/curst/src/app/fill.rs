//! the secrets a collection needs, filled in from curst: after a bundle is
//! imported, and when a request needs a secret that has no value yet
//!
//! values are typed blind, kept in memory until the end and then stored
//! together, so a passphrase asked midway cannot lose what was typed

use curst_core::model::{Request, Variable};
use curst_core::secrets::{Secret, SecretKey};
use ratatui_textarea::TextArea;

use super::editor::{masked, text_of};
use super::workspace::Workspace;
use super::{Action, Command, Popup, SecretChange, State};

/// the open pop-up
#[derive(Debug)]
pub(crate) struct Fill {
    /// each secret, and how the list names it
    pub(crate) secrets: Vec<(SecretKey, String)>,

    /// what was typed for each, if anything
    pub(crate) values: Vec<Option<Secret>>,
    pub(crate) selected: usize,
    pub(crate) input: Box<TextArea<'static>>,

    /// a request to send once the values are stored
    pub(crate) then_send: Option<Request>,
}

fn field() -> Box<TextArea<'static>> {
    Box::new(masked("type the value, or leave it empty to skip"))
}

impl Fill {
    pub(crate) fn new(secrets: Vec<(SecretKey, String)>, then_send: Option<Request>) -> Self {
        Self {
            values: vec![None; secrets.len()],
            secrets,
            selected: 0,
            input: field(),
            then_send,
        }
    }

    /// keeps what is typed for the selected secret, if anything
    fn keep_typed(&mut self) {
        let typed = text_of(&self.input);

        if !typed.is_empty()
            && let Some(slot) = self.values.get_mut(self.selected)
        {
            *slot = Some(Secret::new(typed));
        }

        self.input = field();
    }

    pub(crate) fn filled(&self) -> usize {
        self.values.iter().filter(|v| v.is_some()).count()
    }
}

/// how the list names a secret: its name, and where it belongs
pub(crate) fn label(workspace: Option<&Workspace>, key: &SecretKey) -> String {
    let name = key.name().map_or_else(String::new, ToString::to_string);

    match key {
        SecretKey::Environment { environment, .. } => {
            let place = workspace
                .and_then(|w| w.collection.environment(environment))
                .map_or("an environment", |e| e.name.as_str());

            format!("{name} · {place}")
        }
        SecretKey::Collection { .. } => format!("{name} · the collection"),
        SecretKey::Global { .. } | SecretKey::Probe => format!("{name} · global"),
    }
}

/// every secret a collection's variables name, in its environments first
pub(crate) fn secrets_of(workspace: &Workspace) -> Vec<(SecretKey, String)> {
    let secret = |v: &&Variable| v.secret;
    let mut keys = Vec::new();

    for environment in &workspace.collection.environments {
        for variable in environment.variables.iter().filter(secret) {
            keys.push(SecretKey::Environment {
                collection: workspace.id.clone(),
                environment: environment.id.clone(),
                name: variable.name.clone(),
            });
        }
    }

    for variable in workspace.collection.variables.iter().filter(secret) {
        keys.push(SecretKey::Collection {
            collection: workspace.id.clone(),
            name: variable.name.clone(),
        });
    }

    keys.into_iter()
        .map(|key| {
            let label = label(Some(workspace), &key);

            (key, label)
        })
        .collect()
}

/// opens the pop-up for `secrets`, when there are any
pub(crate) fn open(
    state: &mut State,
    secrets: Vec<(SecretKey, String)>,
    then_send: Option<Request>,
) {
    if !secrets.is_empty() {
        state.popup = Some(Popup::Fill(Box::new(Fill::new(secrets, then_send))));
    }
}

/// stores what was typed, and sends the waiting request once it is stored
fn finish(state: &mut State, fill: Fill) -> Vec<Command> {
    let changes: Vec<SecretChange> = fill
        .secrets
        .into_iter()
        .zip(fill.values)
        .filter_map(|((key, _), value)| Some(SecretChange::Set(key, value?)))
        .collect();

    if changes.is_empty() {
        state.notice = Some("no secret was set; the variable editor can set them later".into());

        return Vec::new();
    }

    super::send::change_secrets_then_send(state, changes, fill.then_send)
}

/// a key in the pop-up: Enter keeps the value and goes on, the arrows and
/// Tab move without it, Esc stores what was typed so far
pub(crate) fn apply(
    state: &mut State,
    mut fill: Box<Fill>,
    action: Action,
) -> Result<Vec<Command>, Action> {
    let last = fill.secrets.len().saturating_sub(1);

    match action {
        Action::Type(key) => {
            fill.input.input(key);
        }
        Action::ConfirmPopup => {
            fill.keep_typed();

            // on to the next one without a value, or done when none is left
            let next = (fill.selected + 1..fill.secrets.len()).find(|i| fill.values[*i].is_none());

            match next {
                Some(next) => fill.selected = next,
                None => return Ok(finish(state, *fill)),
            }
        }
        Action::Down | Action::NextCell => {
            fill.keep_typed();
            fill.selected = (fill.selected + 1).min(last);
        }
        Action::Up => {
            fill.keep_typed();
            fill.selected = fill.selected.saturating_sub(1);
        }
        Action::Pick(index) => {
            fill.keep_typed();
            fill.selected = index.min(last);
        }
        Action::ClosePopup | Action::Save => {
            fill.keep_typed();

            return Ok(finish(state, *fill));
        }
        other => {
            state.popup = Some(Popup::Fill(fill));

            return Err(other);
        }
    }

    state.popup = Some(Popup::Fill(fill));

    Ok(Vec::new())
}
