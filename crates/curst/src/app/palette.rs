//! the command palette: every action and every saved request, found by
//! typing a few of their letters

use curst_core::model::{Item, LocalId, Method};
use ratatui_textarea::TextArea;

use super::editor::{single_line, text_of};
use super::keymap::PaletteCommand;
use super::workspace::NodeKey;
use super::{Action, Command, Layer, Popup, State};

/// the open palette
#[derive(Debug)]
pub(crate) struct Palette {
    pub(crate) input: Box<TextArea<'static>>,
    pub(crate) selected: usize,

    /// the chain of layers where the focus was, whose actions it offers
    pub(crate) here: Vec<Layer>,
}

/// one thing the palette can do
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Entry {
    Run(PaletteCommand),
    Open {
        key: NodeKey,
        method: Method,

        /// the collection, folders and name, as `Shop › Orders › List`
        title: String,
    },
    Workflow {
        collection: LocalId,
        index: usize,

        /// the collection and the workflow, as `Shop › Log in`
        title: String,
    },
}

impl Entry {
    /// the text the query is matched against
    fn text(&self) -> String {
        match self {
            Self::Run(command) => format!("{} {}", command.what, command.name),
            Self::Open { method, title, .. } => format!("{title} {}", method.as_str()),
            Self::Workflow { title, .. } => format!("{title} run workflow"),
        }
    }
}

/// how well `query` matches `text`, higher is better; `None` when its
/// letters do not all appear in `text`, in order. letters next to each
/// other and at the start of words count most
pub(crate) fn score(query: &str, text: &str) -> Option<i64> {
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut at = 0;
    let mut previous: Option<usize> = None;

    for wanted in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let found = (at..text.len()).find(|i| text[*i] == wanted)?;
        let word_start = found == 0 || !text[found - 1].is_alphanumeric();

        score += 1;

        if word_start {
            score += 8;
        }

        match previous {
            Some(p) if p + 1 == found => score += 5,
            Some(p) => score -= i64::try_from((found - p).min(10)).unwrap_or(10),
            None => score -= i64::try_from(found.min(10)).unwrap_or(10),
        }

        previous = Some(found);
        at = found + 1;
    }

    Some(score)
}

fn requests(items: &[Item], parent: &NodeKey, path: &str, out: &mut Vec<Entry>) {
    for item in items {
        let key = parent.child(item.slug());

        match item {
            Item::Folder(folder) => {
                requests(
                    &folder.items,
                    &key,
                    &format!("{path} › {}", folder.name),
                    out,
                );
            }
            Item::Request(saved) => out.push(Entry::Open {
                key,
                method: saved.request.method,
                title: format!("{path} › {}", saved.name),
            }),
        }
    }
}

impl State {
    /// what the palette shows for its query, best match first
    pub(crate) fn palette_entries(&self, palette: &Palette) -> Vec<Entry> {
        let mut entries: Vec<Entry> = self
            .keymap
            .commands(&palette.here)
            .into_iter()
            .map(Entry::Run)
            .collect();

        for workspace in &self.collections {
            let root = NodeKey::root(workspace.id.clone());

            requests(
                &workspace.collection.items,
                &root,
                &workspace.collection.name,
                &mut entries,
            );

            if workspace.trusted {
                entries.extend(workspace.collection.workflows.iter().enumerate().map(
                    |(index, workflow)| Entry::Workflow {
                        collection: workspace.id.clone(),
                        index,
                        title: format!("{} › {}", workspace.collection.name, workflow.name),
                    },
                ));
            }
        }

        let query = text_of(&palette.input);

        if query.trim().is_empty() {
            return entries;
        }

        let mut scored: Vec<(i64, Entry)> = entries
            .into_iter()
            .filter_map(|entry| Some((score(&query, &entry.text())?, entry)))
            .collect();

        // stable, so equal scores keep actions before requests
        scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        scored.into_iter().map(|(_, entry)| entry).collect()
    }
}

/// opens the palette over where the focus is
pub(crate) fn open(state: &mut State) {
    state.popup = Some(Popup::Palette(Box::new(Palette {
        input: Box::new(single_line("", "type to search actions and requests")),
        selected: 0,
        here: super::keymap::layers(state).0,
    })));
}

/// a key while the palette is open; the action it chose is handed back,
/// to run as if its key had been pressed
pub(super) fn apply(
    state: &mut State,
    mut palette: Box<Palette>,
    action: Action,
) -> Result<Vec<Command>, Action> {
    let count = state.palette_entries(&palette).len();

    // a click on an entry runs it, like choosing it and Enter
    let action = match action {
        Action::Pick(index) => {
            palette.selected = index;

            Action::ConfirmPopup
        }
        action => action,
    };

    match action {
        Action::Type(key) => {
            palette.input.input(key);
            palette.selected = 0;
        }
        Action::Place { row, col, .. } => super::mouse::place(&mut palette.input, row, col),
        Action::Up => {
            palette.selected = palette.selected.checked_sub(1).unwrap_or(count.max(1) - 1);
        }
        Action::Down => palette.selected = (palette.selected + 1) % count.max(1),
        Action::ClosePopup | Action::Palette => return Ok(Vec::new()),
        Action::ConfirmPopup => {
            let entry = state
                .palette_entries(&palette)
                .into_iter()
                .nth(palette.selected);

            return match entry {
                Some(Entry::Run(command)) => {
                    let name = command.name.split_once('.').map_or("", |(_, name)| name);

                    super::keymap::action(name).map_or(Ok(Vec::new()), Err)
                }
                Some(Entry::Open { key, .. }) => {
                    super::tabs::open_request(state, &key);

                    Ok(Vec::new())
                }
                Some(Entry::Workflow {
                    collection, index, ..
                }) => Err(Action::RunWorkflow { collection, index }),
                None => {
                    state.popup = Some(Popup::Palette(palette));

                    Ok(Vec::new())
                }
            };
        }
        other => {
            state.popup = Some(Popup::Palette(palette));

            return Err(other);
        }
    }

    state.popup = Some(Popup::Palette(palette));

    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_must_appear_in_order() {
        assert!(score("snd", "send the request").is_some());
        assert!(score("SEND", "send the request").is_some());
        assert!(score("dns", "send the request").is_none());
        assert!(score("x", "send").is_none());
        assert_eq!(score("", "send"), Some(0));
    }

    #[test]
    fn word_starts_and_runs_win() {
        let start = score("env", "environments").unwrap();
        let middle = score("env", "lenvy").unwrap();
        let spread = score("env", "exnxv").unwrap();

        assert!(start > middle, "{start} {middle}");
        assert!(middle > spread, "{middle} {spread}");
        assert!(score("nr", "new request").unwrap() > score("nr", "manner").unwrap());
    }
}
