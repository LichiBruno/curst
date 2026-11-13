//! saving a new request: its name, and where it goes, any collection or
//! folder, picked from a list without leaving the dialog

use curst_core::model::Item;
use ratatui_textarea::TextArea;

use super::editor::{single_line, text_of};
use super::popup::Popup;
use super::workspace::NodeKey;
use super::{Action, Change, Command, State};

/// the open dialog
#[derive(Debug)]
pub(crate) struct SaveDialog {
    pub(crate) input: Box<TextArea<'static>>,

    /// every collection and folder, each with how the list names it
    pub(crate) places: Vec<(NodeKey, String)>,

    /// where the request goes
    pub(crate) place: usize,

    /// whether the list is open, and the place it has selected
    pub(crate) picking: bool,
    pub(crate) selected: usize,
}

fn folders(items: &[Item], parent: &NodeKey, label: &str, places: &mut Vec<(NodeKey, String)>) {
    for item in items {
        if let Item::Folder(folder) = item {
            let key = parent.child(&folder.slug);
            let label = format!("{label} › {}", folder.name);

            places.push((key.clone(), label.clone()));
            folders(&folder.items, &key, &label, places);
        }
    }
}

/// every place a request can be saved: each trusted collection, then its
/// folders, nested ones included
fn places(state: &State) -> Vec<(NodeKey, String)> {
    let mut places = Vec::new();

    for workspace in state.collections.iter().filter(|w| w.trusted) {
        let root = NodeKey::root(workspace.id.clone());
        let name = workspace.collection.name.clone();

        places.push((root.clone(), name.clone()));
        folders(&workspace.collection.items, &root, &name, &mut places);
    }

    places
}

/// opens the dialog, named after the request, going where the sidebar's
/// selection is
pub(crate) fn open(state: &mut State, name: &str) {
    let places = places(state);

    if places.is_empty() {
        state.notice = Some(super::no_collection(state));

        return;
    }

    let target = super::sidebar::target_folder(state);
    let place = target
        .and_then(|target| places.iter().position(|(key, _)| *key == target))
        .unwrap_or(0);

    state.popup = Some(Popup::SaveRequest(Box::new(SaveDialog {
        input: Box::new(single_line(name, "name")),
        places,
        place,
        picking: false,
        selected: place,
    })));
}

/// a key in the dialog: typing names the request, Tab opens and closes the
/// list of places, Enter picks one or saves
pub(crate) fn apply(
    state: &mut State,
    mut dialog: Box<SaveDialog>,
    action: Action,
) -> Result<Vec<Command>, Action> {
    let last = dialog.places.len().saturating_sub(1);

    match action {
        Action::Type(key) if !dialog.picking => {
            dialog.input.input(key);
        }
        Action::Place { row, col, .. } => {
            super::mouse::place(&mut dialog.input, row, col);
            dialog.picking = false;
        }
        Action::NextTab | Action::PreviousTab => {
            dialog.picking = !dialog.picking;
            dialog.selected = dialog.place;
        }
        Action::Up if dialog.picking => dialog.selected = dialog.selected.saturating_sub(1),
        Action::Down if dialog.picking => dialog.selected = (dialog.selected + 1).min(last),
        Action::Pick(index) => {
            dialog.place = index.min(last);
            dialog.selected = dialog.place;
            dialog.picking = false;
        }
        Action::ConfirmPopup if dialog.picking => {
            dialog.place = dialog.selected;
            dialog.picking = false;
        }
        Action::ConfirmPopup => return Ok(save(state, *dialog)),
        Action::ClosePopup if dialog.picking => dialog.picking = false,
        Action::ClosePopup => return Ok(Vec::new()),
        other => {
            state.popup = Some(Popup::SaveRequest(dialog));

            return Err(other);
        }
    }

    state.popup = Some(Popup::SaveRequest(dialog));

    Ok(Vec::new())
}

/// saves the request where the dialog says, under its name
fn save(state: &mut State, dialog: SaveDialog) -> Vec<Command> {
    let name = text_of(&dialog.input).trim().to_owned();

    if name.is_empty() {
        state.notice = Some("give the request a name".into());
        state.popup = Some(Popup::SaveRequest(Box::new(dialog)));

        return Vec::new();
    }

    let Some((parent, _)) = dialog.places.get(dialog.place) else {
        return Vec::new();
    };

    vec![Command::Store {
        collection: parent.collection.clone(),
        change: Change::CreateRequest {
            parent: parent.path.clone(),
            name,
            request: state.request(),
            open: true,
        },
    }]
}
