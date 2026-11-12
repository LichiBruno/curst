use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::*;
use crate::app::{Action, Command, ResponseTab, update};

fn render(state: &State, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| view(state, frame)).unwrap();

    terminal.backend().to_string()
}

fn sent_state(url: &str) -> (State, u64) {
    let mut state = State::default();
    state.url.insert_str(url);
    let commands = update(&mut state, Action::Send);
    let Some(Command::Send { id, .. }) = commands.last() else {
        panic!("no send");
    };
    (state, *id)
}

#[test]
fn empty_screen() {
    insta::assert_snapshot!(render(&State::default(), 80, 16));
}

#[test]
fn json_response_side_by_side() {
    let (mut state, id) = sent_state("http://localhost/pets");
    update(
        &mut state,
        Action::responded(
            id,
            Ok(crate::app::tests::response(
                r#"{"id":1,"name":"Rex","tags":["good"]}"#,
            )),
        ),
    );

    insta::assert_snapshot!(render(&state, 120, 14));
}

#[test]
fn timing_tab() {
    let (mut state, id) = sent_state("http://localhost/");
    update(
        &mut state,
        Action::responded(id, Ok(crate::app::tests::response("{}"))),
    );
    state.response_tab = ResponseTab::Timing;

    insta::assert_snapshot!(render(&state, 80, 22));
    // 22 rows: URL bar 3, footer 1, request pane 8, response pane 10 with
    // its two borders
    assert_eq!(state.response_height.get(), 8);
}

#[test]
fn error_and_sending() {
    let (mut state, id) = sent_state("http://localhost:1/");

    insta::assert_snapshot!("sending", render(&state, 80, 14));

    update(
        &mut state,
        Action::responded(
            id,
            Err(curst_core::http::HttpError::Connect(
                "Failed to connect to localhost port 1".into(),
            )),
        ),
    );

    insta::assert_snapshot!("error", render(&state, 80, 14));
}

#[test]
fn editing_a_header_row() {
    let mut state = State::default();
    update(&mut state, Action::FocusNext);
    update(&mut state, Action::NextTab);
    update(&mut state, Action::AddRow);

    state
        .headers
        .editing
        .as_mut()
        .unwrap()
        .1
        .insert_str("Accept");
    update(&mut state, Action::NextCell);

    insta::assert_snapshot!(render(&state, 80, 12));
    assert_eq!(
        hints(&state),
        [("Enter", "done"), ("Tab", "next field"), ("Ctrl+C", "quit")]
    );
}

#[test]
fn auth_and_save_prompt() {
    let (mut state, id) = sent_state("http://localhost/");
    update(
        &mut state,
        Action::responded(id, Ok(crate::app::tests::response("{}"))),
    );
    state.focus = crate::app::Focus::Request;
    state.request_tab = RequestTab::Auth;
    update(&mut state, Action::CycleKind);
    update(&mut state, Action::Edit);
    state.auth.username.insert_str("me");
    update(&mut state, Action::NextCell);
    state.auth.password.insert_str("secret");

    insta::assert_snapshot!("basic_auth", render(&state, 80, 12));

    state.focus = crate::app::Focus::Response;
    update(&mut state, Action::OpenSave);

    insta::assert_snapshot!("save_prompt", render(&state, 80, 12));
}
