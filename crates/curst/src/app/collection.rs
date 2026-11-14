//! the settings of a collection or a folder: the auth its requests
//! inherit, and for a collection whether its edits are saved without
//! asking, refreshing it from its spec, and exporting it

use curst_core::model::{Auth, Item};
use ratatui_textarea::TextArea;

use super::editor::{AuthEditor, AuthKind};
use super::popup::Popup;
use super::workspace::NodeKey;
use super::{Action, Change, Command, State};

/// the rows of the pop-up, in order
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    Auth,
    Workflows,
    AutoSave,
    Refresh,
    Source,
    Export,
}

impl Row {
    const COLLECTION: [Self; 6] = [
        Self::Auth,
        Self::Workflows,
        Self::AutoSave,
        Self::Refresh,
        Self::Source,
        Self::Export,
    ];
    const FOLDER: [Self; 1] = [Self::Auth];

    /// the rows the settings of `target` have
    pub(crate) fn of(target: &NodeKey) -> &'static [Self] {
        if target.path.is_empty() {
            &Self::COLLECTION
        } else {
            &Self::FOLDER
        }
    }
}

/// the auth of a collection or a folder, being edited
#[derive(Debug)]
pub(crate) struct AuthDialog {
    pub(crate) target: NodeKey,
    pub(crate) editor: AuthEditor,

    /// the type on row 0, then each field
    pub(crate) row: usize,
}

impl AuthDialog {
    /// the field on the selected row, if it is one
    pub(crate) fn field(&mut self) -> Option<&mut TextArea<'static>> {
        let index = self.row.checked_sub(1)?;

        self.editor.selected = index;
        self.editor.selected_field()
    }
}

/// the auth `target` sets, a folder or the whole collection
pub(crate) fn auth_of(state: &State, target: &NodeKey) -> Option<Auth> {
    let workspace = state.workspace(&target.collection)?;

    if target.path.is_empty() {
        return Some(workspace.collection.auth.clone());
    }

    match workspace.collection.item(&target.path)? {
        Item::Folder(folder) => Some(folder.auth.clone()),
        Item::Request(_) => None,
    }
}

/// opens the settings of the folder or collection under the sidebar's
/// selection; a request's are its folder's
pub(crate) fn open(state: &mut State) {
    let selected = state.sidebar.selected.clone().map(|key| {
        let folder = state
            .workspace(&key.collection)
            .and_then(|w| w.collection.item(&key.path))
            .is_some_and(|item| matches!(item, Item::Folder(_)));

        if folder || key.path.is_empty() {
            key
        } else {
            key.parent().unwrap_or(key)
        }
    });
    let target = selected
        .or_else(|| state.collection.clone().map(NodeKey::root))
        .filter(|key| auth_of(state, key).is_some());

    match target {
        Some(target) => {
            state.popup = Some(Popup::CollectionSettings {
                target,
                selected: 0,
            });
        }
        None => state.notice = Some(super::no_collection(state)),
    }
}

/// a key in the pop-up: the arrows choose a row, Enter or Space acts on it
pub(crate) fn apply(
    state: &mut State,
    target: &NodeKey,
    selected: usize,
    action: Action,
) -> Result<Vec<Command>, Action> {
    let last = Row::of(target).len() - 1;
    let keep = |state: &mut State, selected: usize| {
        state.popup = Some(Popup::CollectionSettings {
            target: target.clone(),
            selected,
        });
    };

    match action {
        Action::Up => keep(state, selected.saturating_sub(1)),
        Action::Down => keep(state, (selected + 1).min(last)),
        Action::ClosePopup => {}
        Action::Pick(index) => return Ok(act(state, target, index.min(last))),
        Action::ConfirmPopup | Action::ToggleRow => return Ok(act(state, target, selected)),
        other => {
            keep(state, selected);

            return Err(other);
        }
    }

    Ok(Vec::new())
}

/// does what row `index` says, for `target`
fn act(state: &mut State, target: &NodeKey, index: usize) -> Vec<Command> {
    let collection = &target.collection;

    match Row::of(target)[index] {
        Row::Auth => {
            open_auth(state, target);

            Vec::new()
        }
        Row::Workflows => {
            super::workflow::list(state, collection);

            Vec::new()
        }
        Row::AutoSave => {
            let commands = toggle_auto_save(state, target);

            // the pop-up stays, to show the new state
            state.popup = Some(Popup::CollectionSettings {
                target: target.clone(),
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
fn toggle_auto_save(state: &mut State, target: &NodeKey) -> Vec<Command> {
    let collection = &target.collection;
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

fn open_auth(state: &mut State, target: &NodeKey) {
    let Some(auth) = auth_of(state, target) else {
        return;
    };

    // a collection has nothing above it: inheriting there is none
    let auth = if target.path.is_empty() && auth == Auth::Inherit {
        Auth::None
    } else {
        auth
    };

    state.popup = Some(Popup::Auth(Box::new(AuthDialog {
        target: target.clone(),
        editor: AuthEditor::from_auth(&auth),
        row: 0,
    })));
}

/// a key in the auth dialog: the arrows choose a row, Space changes the
/// type, typing fills the selected field, Enter saves
pub(crate) fn auth_key(
    state: &mut State,
    mut dialog: Box<AuthDialog>,
    action: Action,
) -> Result<Vec<Command>, Action> {
    let last = dialog.editor.kind.field_count();

    match action {
        Action::Up => dialog.row = dialog.row.saturating_sub(1),
        Action::Down | Action::NextCell => dialog.row = (dialog.row + 1).min(last),
        Action::Pick(index) => dialog.row = index.min(last),
        Action::ToggleRow | Action::CycleKind if dialog.row == 0 => {
            let mut kind = dialog.editor.kind.next();

            // a collection inherits from nothing
            if dialog.target.path.is_empty() && kind == AuthKind::Inherit {
                kind = kind.next();
            }

            dialog.editor.kind = kind;
            dialog.editor.selected = 0;
        }
        Action::Type(key) => {
            if let Some(field) = dialog.field() {
                field.input(key);
            }
        }
        Action::Place { row, col, .. } => {
            if let Some(field) = dialog.field() {
                super::mouse::place(field, row, col);
            }
        }
        Action::ConfirmPopup => {
            let auth = dialog.editor.auth();
            let target = dialog.target;

            return Ok(vec![Command::Store {
                collection: target.collection.clone(),
                change: Change::SaveAuth {
                    path: target.path,
                    auth,
                },
            }]);
        }
        Action::ClosePopup => return Ok(Vec::new()),
        other => {
            state.popup = Some(Popup::Auth(dialog));

            return Err(other);
        }
    }

    state.popup = Some(Popup::Auth(dialog));

    Ok(Vec::new())
}
