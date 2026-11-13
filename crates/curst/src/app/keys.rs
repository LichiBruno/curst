use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use curst_core::store::Backend;

use super::{Action, Focus, Popup, RequestTab, State};

pub(crate) fn action_for(state: &State, key: KeyEvent) -> Option<Action> {
    if key.kind == KeyEventKind::Release {
        return None;
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // Ctrl+C always reaches the app, so two presses quit from anywhere
    if ctrl && key.code == KeyCode::Char('c') {
        return Some(Action::Quit);
    }

    if let Some(popup) = &state.popup {
        return popup_key(popup, key);
    }

    match key.code {
        KeyCode::Char('r') if ctrl => return Some(Action::Send),
        KeyCode::Char('s') if ctrl => return Some(Action::Save),
        KeyCode::Char('e') if ctrl => return Some(Action::Environments),
        KeyCode::Char('o') if ctrl => return Some(Action::Settings),
        _ => {}
    }

    match state.focus {
        Focus::Url => Some(match key.code {
            KeyCode::Enter => Action::Send,
            KeyCode::Tab => Action::FocusNext,
            KeyCode::BackTab => Action::FocusPrevious,
            KeyCode::Esc => Action::Cancel,
            _ => Action::Type(key),
        }),
        Focus::Request if state.request_typing() => Some(typing_in_request(state, key)),
        Focus::Sidebar => common(state, key).or_else(|| sidebar_key(key)),
        Focus::Method => common(state, key).or(match key.code {
            KeyCode::Left | KeyCode::Up => Some(Action::PreviousMethod),
            KeyCode::Right | KeyCode::Down | KeyCode::Enter | KeyCode::Char(' ') => {
                Some(Action::NextMethod)
            }
            _ => None,
        }),
        Focus::Request => common(state, key).or(match key.code {
            KeyCode::Enter => Some(Action::Edit),
            KeyCode::Char('c') => Some(Action::EditContentType),
            KeyCode::Char('a') => Some(Action::AddRow),
            KeyCode::Char('d') | KeyCode::Delete => Some(Action::DeleteRow),
            KeyCode::Char(' ') => Some(Action::ToggleRow),
            KeyCode::Char('t') => Some(Action::CycleKind),
            KeyCode::Char('m') => Some(Action::MakeSecret),
            KeyCode::Char('E') => Some(Action::EditBody),
            _ => None,
        }),
        Focus::Response => pan(key).or_else(|| common(state, key)).or(match key.code {
            KeyCode::PageUp => Some(Action::PageUp),
            KeyCode::PageDown => Some(Action::PageDown),
            KeyCode::Home | KeyCode::Char('g') => Some(Action::Top),
            KeyCode::End | KeyCode::Char('G') => Some(Action::Bottom),
            KeyCode::Char('s') => Some(Action::OpenSave),
            KeyCode::Char('f') => Some(Action::Follow),
            _ => None,
        }),
    }
}

fn sidebar_key(key: KeyEvent) -> Option<Action> {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    match key.code {
        KeyCode::Up if shift => Some(Action::MoveUp),
        KeyCode::Down if shift => Some(Action::MoveDown),
        KeyCode::Char('K') => Some(Action::MoveUp),
        KeyCode::Char('J') => Some(Action::MoveDown),
        KeyCode::Up | KeyCode::Char('k') => Some(Action::Up),
        KeyCode::Down | KeyCode::Char('j') => Some(Action::Down),
        KeyCode::Left | KeyCode::Char('h') => Some(Action::Left),
        KeyCode::Right | KeyCode::Char('l') => Some(Action::Right),
        KeyCode::Home => Some(Action::Top),
        KeyCode::End => Some(Action::Bottom),
        KeyCode::Enter => Some(Action::Edit),
        KeyCode::Char('n') => Some(Action::NewRequest),
        KeyCode::Char('f') => Some(Action::NewFolder),
        KeyCode::Char('c') => Some(Action::NewCollection),
        KeyCode::Char('r') => Some(Action::Rename),
        KeyCode::Char('d') | KeyCode::Delete => Some(Action::DeleteItem),
        KeyCode::Char('x') => Some(Action::Cut),
        KeyCode::Char('p') => Some(Action::Paste),
        KeyCode::Char('T') => Some(Action::Trust),
        _ => None,
    }
}

fn popup_key(popup: &Popup, key: KeyEvent) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    match popup {
        Popup::Prompt { .. } | Popup::Passphrase { .. } => Some(match key.code {
            KeyCode::Esc => Action::ClosePopup,
            KeyCode::Enter => Action::ConfirmPopup,
            _ => Action::Type(key),
        }),
        Popup::Trust { .. } => match key.code {
            KeyCode::Char('y' | 'Y') | KeyCode::Enter => Some(Action::ConfirmPopup),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(Action::ClosePopup),
            KeyCode::Char(c @ '1'..='9') => Some(Action::Reattach(usize::from(c as u8 - b'1'))),
            _ => None,
        },
        Popup::ChooseBackend => match key.code {
            KeyCode::Char('k') => Some(Action::Choose(Backend::Keychain)),
            KeyCode::Char('f') => Some(Action::Choose(Backend::EncryptedFile)),
            KeyCode::Esc => Some(Action::ClosePopup),
            _ => None,
        },
        Popup::Settings => match key.code {
            KeyCode::Char('m') => Some(Action::Migrate),
            KeyCode::Esc | KeyCode::Char('q') => Some(Action::ClosePopup),
            _ => None,
        },
        Popup::Approve(_) => match key.code {
            KeyCode::Char('y' | 'Y') => Some(Action::ConfirmPopup),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(Action::ClosePopup),
            _ => None,
        },
        Popup::Confirm { .. } => match key.code {
            KeyCode::Char('y' | 'Y') | KeyCode::Enter => Some(Action::ConfirmPopup),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(Action::ClosePopup),
            _ => None,
        },
        Popup::Environments { .. } => match key.code {
            KeyCode::Up | KeyCode::Char('k') => Some(Action::Up),
            KeyCode::Down | KeyCode::Char('j') => Some(Action::Down),
            KeyCode::Enter => Some(Action::ConfirmPopup),
            KeyCode::Esc | KeyCode::Char('q') => Some(Action::ClosePopup),
            KeyCode::Char('e') if ctrl => Some(Action::ClosePopup),
            KeyCode::Char('n') => Some(Action::NewEnvironment),
            KeyCode::Char('r') => Some(Action::RenameEnvironment),
            KeyCode::Char('d') | KeyCode::Delete => Some(Action::DeleteEnvironment),
            KeyCode::Char('v') => Some(Action::EditVariables),
            _ => None,
        },
        Popup::Variables(editor) if editor.editing() => Some(match key.code {
            KeyCode::Char('s') if ctrl => Action::Save,
            KeyCode::Esc | KeyCode::Enter => Action::StopEdit,
            KeyCode::Tab => Action::NextCell,
            _ => Action::Type(key),
        }),
        Popup::Variables(_) => match key.code {
            KeyCode::Char('s') if ctrl => Some(Action::Save),
            KeyCode::Esc => Some(Action::ClosePopup),
            KeyCode::Left | KeyCode::BackTab => Some(Action::PreviousTab),
            KeyCode::Right | KeyCode::Tab => Some(Action::NextTab),
            KeyCode::Up => Some(Action::Up),
            KeyCode::Down => Some(Action::Down),
            KeyCode::Enter => Some(Action::Edit),
            KeyCode::Char('a') => Some(Action::AddRow),
            KeyCode::Char('d') | KeyCode::Delete => Some(Action::DeleteRow),
            KeyCode::Char(' ') => Some(Action::ToggleRow),
            KeyCode::Char('s') => Some(Action::MakeSecret),
            _ => None,
        },
    }
}

/// shift with an arrow, or `h` and `l`, scroll the response sideways
fn pan(key: KeyEvent) -> Option<Action> {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    match key.code {
        KeyCode::Left if shift => Some(Action::Left),
        KeyCode::Right if shift => Some(Action::Right),
        KeyCode::Char('h') => Some(Action::Left),
        KeyCode::Char('l') => Some(Action::Right),
        _ => None,
    }
}

/// keys that mean the same in every pane when nothing is being typed
fn common(state: &State, key: KeyEvent) -> Option<Action> {
    let panes = matches!(state.focus, Focus::Request | Focus::Response);

    match key.code {
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Tab => Some(Action::FocusNext),
        KeyCode::BackTab => Some(Action::FocusPrevious),
        KeyCode::Esc => Some(Action::Cancel),
        KeyCode::Left if panes => Some(Action::PreviousTab),
        KeyCode::Right if panes => Some(Action::NextTab),
        KeyCode::Up if panes => Some(Action::Up),
        KeyCode::Down if panes => Some(Action::Down),
        _ => None,
    }
}

fn typing_in_request(state: &State, key: KeyEvent) -> Action {
    let multi_line =
        state.request_tab == RequestTab::Body && state.body.typing == Some(super::BodyField::Text);

    match key.code {
        KeyCode::Esc => Action::StopEdit,
        KeyCode::Enter if !multi_line => Action::StopEdit,
        KeyCode::Tab if !multi_line => Action::NextCell,
        _ => Action::Type(key),
    }
}
