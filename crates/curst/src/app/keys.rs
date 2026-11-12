use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::{Action, Focus, RequestTab, State};

pub(crate) fn action_for(state: &State, key: KeyEvent) -> Option<Action> {
    if key.kind == KeyEventKind::Release {
        return None;
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        KeyCode::Char('c') if ctrl => return Some(Action::Quit),
        KeyCode::Char('r') if ctrl => return Some(Action::Send),
        _ => {}
    }

    if state.save_prompt.is_some() {
        return Some(match key.code {
            KeyCode::Esc => Action::StopEdit,
            KeyCode::Enter => Action::ConfirmSave,
            _ => Action::Type(key),
        });
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
        Focus::Method => common(state, key).or(match key.code {
            KeyCode::Left | KeyCode::Up => Some(Action::PreviousMethod),
            KeyCode::Right | KeyCode::Down | KeyCode::Enter | KeyCode::Char(' ') => {
                Some(Action::NextMethod)
            }
            _ => None,
        }),
        Focus::Request => common(state, key).or(match key.code {
            KeyCode::Enter => Some(Action::Edit),
            KeyCode::Char('a') => Some(Action::AddRow),
            KeyCode::Char('d') | KeyCode::Delete => Some(Action::DeleteRow),
            KeyCode::Char(' ') => Some(Action::ToggleRow),
            KeyCode::Char('t') => Some(Action::CycleKind),
            _ => None,
        }),
        Focus::Response => common(state, key).or(match key.code {
            KeyCode::PageUp => Some(Action::PageUp),
            KeyCode::PageDown => Some(Action::PageDown),
            KeyCode::Home | KeyCode::Char('g') => Some(Action::Top),
            KeyCode::End | KeyCode::Char('G') => Some(Action::Bottom),
            KeyCode::Char('s') => Some(Action::OpenSave),
            _ => None,
        }),
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
        state.request_tab == RequestTab::Body && state.body.kind != super::BodyKind::Form;

    match key.code {
        KeyCode::Esc => Action::StopEdit,
        KeyCode::Enter if !multi_line => Action::StopEdit,
        KeyCode::Tab if !multi_line => Action::NextCell,
        _ => Action::Type(key),
    }
}
