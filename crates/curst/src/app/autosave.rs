//! saving edits without asking, in the collections that want it: a short
//! pause in typing saves every tab with edits, and closing a tab or
//! quitting saves it first instead of asking

use curst_core::model::Request;

use super::workspace::NodeKey;
use super::{Change, Command, OpenRequest, State};

/// whether the collection holding `key` saves edits without asking
pub(crate) fn auto_saves(state: &State, key: &NodeKey) -> bool {
    state
        .workspace(&key.collection)
        .is_some_and(|w| w.auto_save && w.trusted)
}

/// the quiet save of `request` over the one at `open`, remembered so a
/// save that fails is not tried again for the same content
fn store(open: &mut OpenRequest, request: Request) -> Command {
    open.tried = Some(request.clone());

    Command::Store {
        collection: open.key.collection.clone(),
        change: Change::SaveRequest {
            path: open.key.path.clone(),
            request,
            quiet: true,
        },
    }
}

/// saves every tab whose edits its collection saves without asking;
/// `again` also saves content a failed save already tried
fn save_all(state: &mut State, again: bool) -> Vec<Command> {
    let wanted = |state: &State, open: &OpenRequest, request: &Request| {
        *request != open.baseline
            && (again || open.tried.as_ref() != Some(request))
            && auto_saves(state, &open.key)
    };
    let mut commands = Vec::new();

    let request = state.request();

    if let Some(open) = &state.open
        && wanted(state, open, &request)
        && let Some(open) = &mut state.open
    {
        commands.push(store(open, request));
    }

    for index in 0..state.others.len() {
        let request = state.others[index].request();

        if let Some(open) = &state.others[index].open
            && wanted(state, open, &request)
            && let Some(open) = &mut state.others[index].open
        {
            commands.push(store(open, request));
        }
    }

    commands
}

/// a pause in typing: saves the tabs with edits, unless a pop-up is open,
/// such as a refresh to review, or secrets are being stored, which a save
/// under it would get ahead of
pub(crate) fn idle(state: &mut State) -> Vec<Command> {
    if state.popup.is_some() || state.waiting.is_some() {
        return Vec::new();
    }

    save_all(state, false)
}

/// saves every tab with edits its collection saves without asking, before
/// closing them or quitting
pub(crate) fn flush(state: &mut State) -> Vec<Command> {
    save_all(state, true)
}

/// the request at `key` was written as `written`: each tab holding it
/// counts it as saved from there, whichever tab is active now
pub(crate) fn written(state: &mut State, key: &NodeKey, written: &Request) {
    let opens = std::iter::once(&mut state.open)
        .chain(state.others.iter_mut().map(|tab| &mut tab.open))
        .flatten()
        .filter(|open| open.key == *key);

    for open in opens {
        open.baseline.clone_from(written);
        open.tried = None;
    }
}
