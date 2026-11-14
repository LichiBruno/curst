//! what the sidebar keys do: moving through the tree, opening requests and
//! changing the collections

use std::path::PathBuf;

use curst_core::model::{Item, LocalId};

use super::popup::{Pending, Popup, Purpose};
use super::workspace::{NodeKey, Row, RowKind, Workspace, rows};
use super::{Action, Change, Command, Layer, State};

fn visible(state: &State) -> Vec<Row> {
    rows(&state.collections, &state.sidebar.expanded)
}

fn selected_row(state: &State) -> Option<Row> {
    let rows = visible(state);
    let selected = state.sidebar.selected.as_ref()?;

    rows.into_iter().find(|r| r.key == *selected)
}

/// the folder or collection new items go into: the selected one, or the
/// folder holding the selected request
pub(crate) fn target_folder(state: &State) -> Option<NodeKey> {
    match selected_row(state) {
        Some(row) if matches!(row.kind, RowKind::Request(_)) => row.key.parent(),
        Some(row) => Some(row.key),
        None => state
            .collections
            .first()
            .map(|w| NodeKey::root(w.id.clone())),
    }
}

fn no_collection(state: &mut State) {
    state.notice = Some(super::no_collection(state));
}

pub(crate) fn apply(state: &mut State, action: &Action) -> Vec<Command> {
    match action {
        Action::Up | Action::Down | Action::Top | Action::Bottom => select(state, action),
        Action::Right | Action::Edit => open(state, matches!(action, Action::Edit)),
        Action::Left => close(state),
        Action::NewRequest | Action::NewFolder => {
            let Some(parent) = target_folder(state) else {
                no_collection(state);

                return Vec::new();
            };

            let purpose = if matches!(action, Action::NewRequest) {
                Purpose::NewRequest(parent)
            } else {
                Purpose::NewFolder(parent)
            };

            state.popup = Some(Popup::prompt(purpose, ""));
        }
        Action::NewCollection => state.popup = Some(Popup::prompt(Purpose::NewCollection, "")),
        Action::Rename => {
            if let Some(row) = selected_row(state) {
                state.popup = Some(Popup::prompt(Purpose::Rename(row.key), &row.name));
            }
        }
        Action::DeleteItem => {
            let unmanaged = selected_row(state)
                .filter(|r| r.kind == RowKind::Collection)
                .and_then(|r| {
                    state
                        .workspace(&r.key.collection)
                        .filter(|w| !w.managed)
                        .map(|w| w.root.display().to_string())
                });

            if let Some(root) = unmanaged {
                state.notice = Some(format!(
                    "curst does not delete {root}; remove that folder yourself"
                ));
            } else if let Some(row) = selected_row(state) {
                let message = match row.kind {
                    RowKind::Collection => {
                        let secrets = state
                            .workspace(&row.key.collection)
                            .map_or(0, |w| super::secret_count(w, None));

                        format!(
                            "Delete the collection {} and every request in it{}?",
                            row.name,
                            super::popup::secrets_too(secrets)
                        )
                    }
                    RowKind::Folder => {
                        format!("Delete the folder {} and everything in it?", row.name)
                    }
                    RowKind::Request(_) => format!("Delete the request {}?", row.name),
                };

                state.popup = Some(Popup::confirm(message, Pending::Delete(row.key)));
            }
        }
        Action::Cut => match selected_row(state) {
            Some(row) if row.kind != RowKind::Collection => {
                state.notice = Some(format!(
                    "{} cut: select a folder and press {}",
                    row.name,
                    state.key(Layer::Sidebar, "paste")
                ));
                state.sidebar.cut = Some(row.key);
            }
            _ => state.notice = Some("select a folder or request to move".into()),
        },
        Action::Paste => return paste(state),
        Action::Trust => match selected_row(state) {
            Some(row) if !row.trusted => {
                state.popup = Some(Popup::Trust {
                    collection: row.key.collection,
                });
            }
            Some(_) => state.notice = Some("this collection is already trusted".into()),
            None => {}
        },
        Action::MoveUp | Action::MoveDown => {
            return reorder(state, matches!(action, Action::MoveUp));
        }
        _ => {}
    }

    Vec::new()
}

fn select(state: &mut State, action: &Action) {
    let rows = visible(state);

    if rows.is_empty() {
        return;
    }

    let current = state
        .sidebar
        .selected
        .as_ref()
        .and_then(|k| rows.iter().position(|r| r.key == *k));
    let index = match (action, current) {
        (Action::Top, _) | (_, None) => 0,
        (Action::Up, Some(i)) => i.saturating_sub(1),
        (Action::Down, Some(i)) => (i + 1).min(rows.len() - 1),
        _ => rows.len() - 1,
    };

    state.sidebar.selected = rows.get(index).map(|r| r.key.clone());
}

/// opens a request into the editor, or expands a folder; Enter also
/// collapses an expanded one
fn open(state: &mut State, toggle: bool) {
    let Some(row) = selected_row(state) else {
        return;
    };

    match row.kind {
        // a request opens in a tab of its own, so nothing open is lost
        RowKind::Request(_) => super::tabs::open_request(state, &row.key),
        RowKind::Folder | RowKind::Collection => {
            if row.expanded && toggle {
                state.sidebar.expanded.remove(&row.key);
            } else {
                state.sidebar.expanded.insert(row.key);
            }
        }
    }
}

fn close(state: &mut State) {
    let Some(row) = selected_row(state) else {
        return;
    };

    if row.expanded {
        state.sidebar.expanded.remove(&row.key);
    } else if let Some(parent) = row.key.parent() {
        state.sidebar.selected = Some(parent);
    }
}

fn paste(state: &mut State) -> Vec<Command> {
    let Some(cut) = state.sidebar.cut.clone() else {
        state.notice = Some(format!(
            "nothing cut: press {} on a folder or request first",
            state.key(Layer::Sidebar, "cut")
        ));

        return Vec::new();
    };

    let Some(target) = target_folder(state) else {
        return Vec::new();
    };

    if target.collection != cut.collection {
        state.notice = Some("requests move only within their collection".into());

        return Vec::new();
    }

    if target.is_within(&cut) {
        state.notice = Some("a folder cannot move into itself".into());

        return Vec::new();
    }

    state.sidebar.cut = None;

    vec![Command::Store {
        collection: cut.collection.clone(),
        change: Change::Move {
            path: cut.path,
            parent: target.path,
        },
    }]
}

fn reorder(state: &mut State, up: bool) -> Vec<Command> {
    let Some(row) = selected_row(state) else {
        return Vec::new();
    };

    let Some(parent) = row.key.parent() else {
        return Vec::new();
    };

    let Some(workspace) = state.workspace(&row.key.collection) else {
        return Vec::new();
    };

    let siblings = if parent.path.is_empty() {
        Some(&workspace.collection.items)
    } else {
        match workspace.collection.item(&parent.path) {
            Some(Item::Folder(folder)) => Some(&folder.items),
            _ => None,
        }
    };
    let Some(siblings) = siblings else {
        return Vec::new();
    };

    let slug = row.key.path.last().map_or("", String::as_str);
    let Some(index) = siblings.iter().position(|i| i.slug() == slug) else {
        return Vec::new();
    };

    let target = if up {
        index.checked_sub(1)
    } else {
        Some(index + 1).filter(|i| *i < siblings.len())
    };

    target.map_or_else(Vec::new, |index| {
        vec![Command::Store {
            collection: row.key.collection.clone(),
            change: Change::Reorder {
                path: row.key.path.clone(),
                index,
            },
        }]
    })
}

/// asks where to write the collection under the selection as a bundle
pub(crate) fn ask_export(state: &mut State) {
    let Some(workspace) = selected_workspace(state) else {
        no_collection(state);

        return;
    };

    let id = workspace.id.clone();

    ask_export_of(state, &id);
}

/// asks where to write `collection` as a bundle
pub(crate) fn ask_export_of(state: &mut State, collection: &LocalId) {
    let Some(workspace) = state.workspace(collection) else {
        return;
    };

    let file = format!(
        "{}.curst.toml",
        curst_core::curl::name_for(&workspace.collection.name)
    );

    state.popup = Some(Popup::prompt(
        Purpose::ExportBundle(collection.clone()),
        &file,
    ));
}

/// the id of the collection of the selected row, or the one the tab
/// belongs to
pub(crate) fn selected_collection(state: &State) -> Option<LocalId> {
    selected_workspace(state).map(|w| w.id.clone())
}

/// the collection of the selected row, or the one the tab belongs to
fn selected_workspace(state: &State) -> Option<&Workspace> {
    let id = state
        .sidebar
        .selected
        .as_ref()
        .map(|key| key.collection.clone())
        .or_else(|| state.collection.clone())?;

    state.workspace(&id)
}

/// exports a collection, asking first when it would write values that
/// look like credentials in plain text
pub(crate) fn export(state: &mut State, collection: LocalId, path: PathBuf) -> Vec<Command> {
    let Some(workspace) = state.workspace(&collection) else {
        state.notice = Some("that collection is gone".into());

        return Vec::new();
    };

    let found = curst_core::store::bundle::plain_credentials(&workspace.collection);

    if let Some(first) = found.first() {
        let more = match found.len() {
            1 => String::new(),
            n => format!(" and {} more", n - 1),
        };

        state.popup = Some(Popup::confirm(
            format!(
                "The bundle would hold credentials in plain text ({first}{more}). Make them secrets first, or export anyway?"
            ),
            Pending::Export(collection, path),
        ));

        return Vec::new();
    }

    vec![Command::ExportBundle { collection, path }]
}
