use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};

#[derive(Debug, Default)]
pub(crate) struct State {
    pub(crate) should_quit: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Quit,
}

pub(crate) fn action_for(key: KeyEvent) -> Option<Action> {
    if key.kind != KeyEventKind::Press {
        return None;
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => Some(Action::Quit),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(Action::Quit),
        _ => None,
    }
}

pub(crate) fn update(state: &mut State, action: Action) {
    match action {
        Action::Quit => state.should_quit = true,
    }
}

pub(crate) fn view(_state: &State, frame: &mut Frame) {
    let [main, footer] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
    frame.render_widget(Block::bordered().title(" curst "), main);
    frame.render_widget(Paragraph::new(Line::from(" q  Quit")), footer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyEventState;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn quit_action_ends_the_loop() {
        let mut state = State::default();
        update(&mut state, Action::Quit);
        assert!(state.should_quit);
    }

    #[test]
    fn quit_keys_map_to_quit() {
        for key in [
            press(KeyCode::Char('q'), KeyModifiers::NONE),
            press(KeyCode::Esc, KeyModifiers::NONE),
            press(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            assert_eq!(action_for(key), Some(Action::Quit), "{key:?}");
        }
    }

    #[test]
    fn other_keys_and_releases_do_nothing() {
        assert_eq!(
            action_for(press(KeyCode::Char('c'), KeyModifiers::NONE)),
            None
        );
        let release = KeyEvent {
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
            ..press(KeyCode::Char('q'), KeyModifiers::NONE)
        };
        assert_eq!(action_for(release), None);
    }

    #[test]
    fn empty_screen() {
        let mut terminal = Terminal::new(TestBackend::new(40, 6)).unwrap();
        terminal
            .draw(|frame| view(&State::default(), frame))
            .unwrap();
        insta::assert_snapshot!(terminal.backend());
    }
}
