//! the settings of one collection: whether its edits are saved without
//! asking, refreshing it from its spec, and exporting it

use curst_core::model::LocalId;

use super::popup::Popup;
use super::{Action, Command, State};

/// the rows of the pop-up, in order
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    AutoSave,
    Refresh,
    Source,
    Export,
}

impl Row {
    pub(crate) const ALL: [Self; 4] = [Self::AutoSave, Self::Refresh, Self::Source, Self::Export];
}

/// opens the settings of the collection under the sidebar's selection, or
/// the one the tab belongs to
pub(crate) fn open(state: &mut State) {
    let chosen = state
        .sidebar
        .selected
        .as_ref()
        .map(|key| key.collection.clone())
        .or_else(|| state.collection.clone())
        .filter(|id| state.workspace(id).is_some());

    match chosen {
        Some(collection) => {
            state.popup = Some(Popup::CollectionSettings {
                collection,
                selected: 0,
            });
        }
        None => state.notice = Some(super::no_collection(state)),
    }
}

/// a key in the pop-up: the arrows choose a row, Enter or Space acts on it
pub(crate) fn apply(
    state: &mut State,
    collection: &LocalId,
    selected: usize,
    action: Action,
) -> Result<Vec<Command>, Action> {
    let last = Row::ALL.len() - 1;
    let keep = |state: &mut State, selected: usize| {
        state.popup = Some(Popup::CollectionSettings {
            collection: collection.clone(),
            selected,
        });
    };

    match action {
        Action::Up => keep(state, selected.saturating_sub(1)),
        Action::Down => keep(state, (selected + 1).min(last)),
        Action::ClosePopup => {}
        Action::Pick(index) => return Ok(act(state, collection, index.min(last))),
        Action::ConfirmPopup | Action::ToggleRow => return Ok(act(state, collection, selected)),
        other => {
            keep(state, selected);

            return Err(other);
        }
    }

    Ok(Vec::new())
}

/// does what row `index` says, for `collection`
fn act(state: &mut State, collection: &LocalId, index: usize) -> Vec<Command> {
    match Row::ALL[index] {
        Row::AutoSave => {
            let commands = toggle_auto_save(state, collection);

            // the pop-up stays, to show the new state
            state.popup = Some(Popup::CollectionSettings {
                collection: collection.clone(),
                selected: index,
            });

            commands
        }
        Row::Refresh => super::refresh::refresh(state, collection.clone()),
        Row::Source => {
            super::refresh::ask_source_of(state, collection.clone());

            Vec::new()
        }
        Row::Export => {
            super::sidebar::ask_export_of(state, collection);

            Vec::new()
        }
    }
}

/// turns saving edits without asking on or off; only a trusted collection
/// can have it, as its files are written without a word
fn toggle_auto_save(state: &mut State, collection: &LocalId) -> Vec<Command> {
    let Some(workspace) = state.workspace_mut(collection) else {
        return Vec::new();
    };

    if !workspace.trusted {
        let key = state.key(super::keymap::Layer::Sidebar, "trust");

        state.notice = Some(format!(
            "trust the collection first: press {key} on it in the sidebar"
        ));

        return Vec::new();
    }

    workspace.auto_save = !workspace.auto_save;

    vec![Command::SetAutoSave {
        collection: collection.clone(),
        on: workspace.auto_save,
    }]
}
