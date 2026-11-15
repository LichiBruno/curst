//! with the vim keys, the command line `:` opens at the bottom of the
//! screen, as vim's: `:q`, `:w`, `:wq` and the rest

use ratatui::crossterm::event::KeyCode;
use ratatui_textarea::TextArea;

use super::editor::{single_line, text_of};
use super::popup::Popup;
use super::{Action, Command, State};

/// a vim command typed after `:`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ex {
    /// `:q`, close the request tab, asking first when it has edits
    Close,

    /// `:q!`, close it without asking
    ForceClose,

    /// `:w`, save the request
    Write,

    /// `:wq` and `:x`, save it and close the tab
    WriteClose,

    /// `:qa`, quit, asking first when anything has edits
    QuitAll,

    /// `:qa!`, quit without asking
    ForceQuitAll,
}

impl Ex {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        Some(match text.trim() {
            "q" | "quit" | "clo" | "close" => Self::Close,
            "q!" | "quit!" => Self::ForceClose,
            "w" | "write" => Self::Write,
            "wq" | "x" | "xit" => Self::WriteClose,
            "qa" | "qall" | "quitall" => Self::QuitAll,
            "qa!" | "qall!" | "quitall!" => Self::ForceQuitAll,
            _ => return None,
        })
    }

    /// runs the command
    pub(crate) fn run(self, state: &mut State) -> Result<Vec<Command>, Action> {
        match self {
            Self::Close => Err(Action::CloseTab),
            Self::ForceClose => Ok(super::tabs::close_now(state)),
            Self::Write => Err(Action::Save),
            Self::WriteClose => {
                // a request never saved asks where to go first, or says why
                // it cannot be; its tab stays
                let saved = state.open.is_some();
                let mut commands = super::save(state);

                if saved {
                    commands.extend(super::tabs::close_now(state));
                }

                Ok(commands)
            }
            Self::QuitAll => Err(Action::Quit),
            Self::ForceQuitAll => {
                state.should_quit = true;

                Ok(Vec::new())
            }
        }
    }
}

/// opens the command line
pub(crate) fn open(state: &mut State) {
    state.popup = Some(Popup::CommandLine(Box::new(single_line("", ""))));
}

/// a key on the command line: Enter runs what is typed, Esc closes it, as
/// does Backspace with nothing left to delete
pub(crate) fn apply(
    state: &mut State,
    mut input: Box<TextArea<'static>>,
    action: Action,
) -> Result<Vec<Command>, Action> {
    match action {
        Action::Type(key) if key.code == KeyCode::Backspace && text_of(&input).is_empty() => {}
        Action::Type(key) => {
            input.input(key);
            state.popup = Some(Popup::CommandLine(input));
        }
        Action::Place { row, col, .. } => {
            super::mouse::place(&mut input, row, col);
            state.popup = Some(Popup::CommandLine(input));
        }
        Action::ConfirmPopup => {
            let typed = text_of(&input);

            return match Ex::parse(&typed) {
                Some(ex) => ex.run(state),
                None if typed.trim().is_empty() => Ok(Vec::new()),
                None => {
                    state.notice = Some(format!("not a command: {}", typed.trim()));

                    Ok(Vec::new())
                }
            };
        }
        Action::ClosePopup => {}
        other => {
            state.popup = Some(Popup::CommandLine(input));

            return Err(other);
        }
    }

    Ok(Vec::new())
}
