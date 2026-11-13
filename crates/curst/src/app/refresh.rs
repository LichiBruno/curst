//! refreshing a collection from its spec: fetched again from where it came
//! from, then every new, changed and removed endpoint offered, each changed
//! one with a diff, for the user to decide on
//!
//! nothing is written until the user says which: new endpoints are offered
//! ticked, changed and removed ones not, since the user's own edits are in
//! them

use curst_core::model::{LocalId, Source};
use curst_core::openapi::Converted;
use curst_core::openapi::refresh::{Choices, LoadError, Plan, plan};
use curst_core::vars::VarName;

use super::popup::{Popup, Purpose};
use super::workspace::NodeKey;
use super::{Action, Change, Command, State};

/// the kind of a line of the review
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Added,
    Changed,
    Removed,
}

/// the review of a refresh: every endpoint that differs, whether it is
/// chosen, and which are held back by unsaved edits in a tab
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Review {
    pub(crate) collection: LocalId,
    pub(crate) plan: Box<Plan>,

    /// each line: its kind and its place in the plan's list of that kind
    pub(crate) lines: Vec<(Kind, usize)>,
    pub(crate) chosen: Vec<bool>,

    /// changed requests open in a tab with unsaved edits: replacing them
    /// would lose the edits or, on the next save, the change
    pub(crate) held: Vec<bool>,
    pub(crate) selected: usize,

    /// the first line shown of the selected change's diff
    pub(crate) scroll: usize,
}

impl Review {
    fn new(collection: LocalId, plan: Plan, unsaved: &[NodeKey]) -> Self {
        let mut lines = Vec::new();

        lines.extend((0..plan.added.len()).map(|i| (Kind::Added, i)));
        lines.extend((0..plan.changed.len()).map(|i| (Kind::Changed, i)));
        lines.extend((0..plan.removed.len()).map(|i| (Kind::Removed, i)));

        let held: Vec<bool> = lines
            .iter()
            .map(|(kind, i)| {
                *kind == Kind::Changed
                    && unsaved.iter().any(|key| {
                        key.collection == collection && key.path == plan.changed[*i].path
                    })
            })
            .collect();
        let chosen = lines.iter().map(|(kind, _)| *kind == Kind::Added).collect();

        Self {
            collection,
            plan: Box::new(plan),
            lines,
            chosen,
            held,
            selected: 0,
            scroll: 0,
        }
    }

    /// the line of the review for one entry
    pub(crate) fn label(&self, index: usize) -> String {
        let Some((kind, i)) = self.lines.get(index) else {
            return String::new();
        };

        match kind {
            Kind::Added => {
                let added = &self.plan.added[*i];
                let folder = added
                    .folder
                    .as_ref()
                    .map_or_else(String::new, |f| format!(" → {f}"));

                format!("new      {} · {}{folder}", added.spec.operation, added.name)
            }
            Kind::Changed => {
                let changed = &self.plan.changed[*i];
                let held = if self.held[index] {
                    "  (unsaved edits in a tab: save or close it first)"
                } else {
                    ""
                };

                format!(
                    "changed  {} · {}{held}",
                    changed.spec.operation, changed.name
                )
            }
            Kind::Removed => {
                let removed = &self.plan.removed[*i];

                format!("gone     {} · {}", removed.operation, removed.name)
            }
        }
    }

    /// what the user chose, as the plan's lists count
    fn choices(&self) -> Choices {
        let mut choices = Choices::default();

        for ((kind, i), chosen) in self.lines.iter().zip(&self.chosen) {
            if !chosen {
                continue;
            }

            match kind {
                Kind::Added => choices.add.push(*i),
                Kind::Changed => choices.replace.push(*i),
                Kind::Removed => choices.delete.push(*i),
            }
        }

        choices
    }
}

/// what happens once a refresh is written
#[derive(Debug, Default)]
pub(crate) struct Refreshing {
    pub(crate) collection: Option<LocalId>,

    /// requests replaced, to reload where a tab shows them unchanged
    pub(crate) replaced: Vec<NodeKey>,

    /// secrets the new endpoints need, to ask for
    pub(crate) secrets: Vec<VarName>,

    /// a collection whose source was just changed to answer a missing
    /// one, to refresh once it is saved
    pub(crate) after_source: Option<LocalId>,
}

/// the collection under the selection, or the one the tab belongs to
fn chosen_collection(state: &State) -> Option<LocalId> {
    state
        .sidebar
        .selected
        .as_ref()
        .map(|key| key.collection.clone())
        .or_else(|| state.collection.clone())
}

/// fetches the spec of the selected collection again, to review the
/// differences
pub(crate) fn start(state: &mut State) -> Vec<Command> {
    let Some(collection) = chosen_collection(state) else {
        state.notice = Some("select a collection imported from a spec".into());

        return Vec::new();
    };

    refresh(state, collection)
}

fn refresh(state: &mut State, collection: LocalId) -> Vec<Command> {
    let Some(workspace) = state.workspace(&collection) else {
        return Vec::new();
    };

    let Some(source) = workspace.collection.source.clone() else {
        state.notice = Some(format!(
            "{} was not imported from a spec, so there is nothing to refresh it from",
            workspace.collection.name
        ));

        return Vec::new();
    };

    super::spec::run(
        state,
        super::spec::Job {
            target: super::spec::Target::Refresh(collection),
            source,
        },
    )
}

/// the spec, loaded again: the review, or a question for where it went
pub(crate) fn loaded(
    state: &mut State,
    id: u64,
    collection: &LocalId,
    result: Result<Box<Converted>, LoadError>,
) {
    if state.refresh_fetch != Some(id) {
        return;
    }

    state.refresh_fetch = None;

    let Some(workspace) = state.workspace(collection) else {
        return;
    };

    match result {
        Err(LoadError::Missing(why)) => {
            let location = workspace
                .collection
                .source
                .as_ref()
                .map_or_else(String::new, |s| s.location.clone());

            state.notice = Some(format!("{why}: where is the spec now?"));
            state.popup = Some(Popup::prompt(
                Purpose::Relocate(collection.clone()),
                &location,
            ));
        }
        Err(LoadError::Failed(why)) => state.notice = Some(format!("not refreshed: {why}")),
        Ok(converted) => {
            let plan = plan(&workspace.collection, &converted);

            if plan.is_empty() {
                state.notice = Some(format!(
                    "{} is up to date with its spec: {} endpoints, nothing new",
                    workspace.collection.name, plan.unchanged
                ));

                return;
            }

            state.notice = None;

            let unsaved = state.unsaved_keys();

            state.popup = Some(Popup::Review(Box::new(Review::new(
                collection.clone(),
                plan,
                &unsaved,
            ))));
        }
    }
}

/// a key in the review: Space ticks or unticks, Enter applies, the page
/// keys scroll the diff
pub(crate) fn review_key(
    state: &mut State,
    mut review: Box<Review>,
    action: Action,
) -> Result<Vec<Command>, Action> {
    let last = review.lines.len().saturating_sub(1);

    match action {
        Action::Up => {
            review.selected = review.selected.saturating_sub(1);
            review.scroll = 0;
        }
        Action::Down => {
            review.selected = (review.selected + 1).min(last);
            review.scroll = 0;
        }
        Action::Pick(index) => {
            review.selected = index.min(last);
            review.scroll = 0;
        }
        Action::PageDown => review.scroll += 5,
        Action::PageUp => review.scroll = review.scroll.saturating_sub(5),
        Action::ToggleRow => {
            let index = review.selected;

            if review.held.get(index) == Some(&true) {
                state.notice =
                    Some("that request has unsaved edits in a tab: save or close it first".into());
            } else if let Some(chosen) = review.chosen.get_mut(index) {
                *chosen = !*chosen;
            }
        }
        Action::ConfirmPopup => return Ok(apply(state, *review)),
        Action::ClosePopup => {
            state.notice = Some("nothing was changed".into());

            return Ok(Vec::new());
        }
        other => {
            state.popup = Some(Popup::Review(review));

            return Err(other);
        }
    }

    state.popup = Some(Popup::Review(review));

    Ok(Vec::new())
}

/// writes what the user chose
fn apply(state: &mut State, review: Review) -> Vec<Command> {
    let choices = review.choices();

    if choices.add.is_empty() && choices.replace.is_empty() && choices.delete.is_empty() {
        state.notice = Some("nothing was chosen, so nothing was changed".into());

        return Vec::new();
    }

    let replaced = choices
        .replace
        .iter()
        .filter_map(|i| review.plan.changed.get(*i))
        .map(|c| NodeKey {
            collection: review.collection.clone(),
            path: c.path.clone(),
        })
        .collect();
    let secrets = if choices.add.is_empty() {
        Vec::new()
    } else {
        review
            .plan
            .variables
            .iter()
            .filter(|v| v.secret)
            .map(|v| v.name.clone())
            .collect()
    };

    state.notice = Some(format!(
        "refreshed: {} added, {} replaced, {} deleted",
        choices.add.len(),
        choices.replace.len(),
        choices.delete.len()
    ));
    state.refreshing.collection = Some(review.collection.clone());
    state.refreshing.replaced = replaced;
    state.refreshing.secrets = secrets;

    vec![Command::Store {
        collection: review.collection,
        change: Change::ApplyRefresh {
            plan: review.plan,
            choices,
        },
    }]
}

/// asks where the spec of the selected collection is, to change it
pub(crate) fn ask_source(state: &mut State) {
    let Some(collection) = chosen_collection(state) else {
        state.notice = Some("select a collection".into());

        return;
    };

    let location = state
        .workspace(&collection)
        .and_then(|w| w.collection.source.as_ref())
        .map_or_else(String::new, |s| s.location.clone());

    state.popup = Some(Popup::prompt(Purpose::ChangeSource(collection), &location));
}

/// records where a collection's spec is now; `then_refresh` refreshes once
/// it is saved
pub(crate) fn set_source(
    state: &mut State,
    collection: LocalId,
    location: &str,
    then_refresh: bool,
) -> Vec<Command> {
    let location = location.trim();

    if location.is_empty() {
        state.notice = Some("give a file, a folder or a URL".into());

        return Vec::new();
    }

    // a curl command gives the headers too, its credentials kept as
    // secrets; otherwise the headers sent with it stay
    let mut secrets = (Vec::new(), Vec::new());
    let source = if super::request::is_curl(location) {
        match super::spec::from_curl(state, location) {
            Ok((stored, _, changes, variables)) => {
                secrets = (changes, variables);

                stored
            }
            Err(why) => {
                state.notice = Some(format!("not changed: {why}"));

                return Vec::new();
            }
        }
    } else {
        Source {
            location: location.to_owned(),
            headers: state
                .workspace(&collection)
                .and_then(|w| w.collection.source.as_ref())
                .map_or_else(Vec::new, |s| s.headers.clone()),
            picked: None,
        }
    };

    if then_refresh {
        state.refreshing.after_source = Some(collection.clone());
    }

    state.notice = Some(format!("the spec is now looked for at {}", source.location));

    // the source is saved once its secrets are, so a refresh after it finds
    // them
    let store = vec![Command::Store {
        collection,
        change: Change::SetSource(source),
    }];

    super::spec::keep_secrets(state, secrets.0, secrets.1, store)
}

/// what follows a refresh or a new source once it is written: tabs that
/// show a replaced request unchanged are reloaded, the secrets new
/// endpoints need asked for, or the refresh that waited for the source
pub(crate) fn after_stored(state: &mut State, collection: &LocalId) -> Vec<Command> {
    if state.refreshing.after_source.as_ref() == Some(collection) {
        state.refreshing.after_source = None;

        return refresh(state, collection.clone());
    }

    if state.refreshing.collection.as_ref() != Some(collection) {
        return Vec::new();
    }

    let replaced = std::mem::take(&mut state.refreshing.replaced);
    let secrets = std::mem::take(&mut state.refreshing.secrets);

    state.refreshing.collection = None;

    for key in &replaced {
        super::tabs::reload_if_unchanged(state, key);
    }

    if let Some(workspace) = state.workspace(collection) {
        let asked = super::fill::secrets_of(workspace)
            .into_iter()
            .filter(|(key, _)| key.name().is_some_and(|n| secrets.contains(n)))
            .collect();

        super::fill::open(state, asked, None);
    }

    Vec::new()
}
