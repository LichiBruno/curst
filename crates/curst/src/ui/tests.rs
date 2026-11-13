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

#[test]
fn text_body_with_its_content_type() {
    let mut state = State::default();

    state.focus = crate::app::Focus::Request;
    state.request_tab = RequestTab::Body;

    update(&mut state, Action::CycleKind);
    update(&mut state, Action::CycleKind);
    update(&mut state, Action::EditContentType);

    insta::assert_snapshot!(render(&state, 80, 12));
    assert_eq!(
        hints(&state),
        [("Enter", "done"), ("Tab", "next field"), ("Ctrl+C", "quit")]
    );

    update(&mut state, Action::StopEdit);

    assert!(hints(&state).contains(&("c", "content type")));
}

#[test]
fn a_wide_response_scrolled_sideways() {
    let (mut state, id) = sent_state("http://localhost/");
    let body = format!(r#"{{"long":"{}","end":true}}"#, "abcdefghij".repeat(8));

    update(
        &mut state,
        Action::responded(id, Ok(crate::app::tests::response(&body))),
    );
    state.focus = crate::app::Focus::Response;
    render(&state, 80, 14);

    for _ in 0..3 {
        update(&mut state, Action::Right);
    }

    insta::assert_snapshot!(render(&state, 80, 14));
    assert!(hints(&state).contains(&("h l", "sideways")));
}

#[test]
fn wide_characters_can_be_scrolled_to_the_end() {
    let (mut state, id) = sent_state("http://localhost/");
    let body = format!(r#"["{}END"]"#, "字".repeat(60));

    update(
        &mut state,
        Action::responded(id, Ok(crate::app::tests::response(&body))),
    );
    state.focus = crate::app::Focus::Response;
    render(&state, 80, 14);

    assert!(state.last_column() > 0);

    for _ in 0..20 {
        update(&mut state, Action::Right);
    }

    assert!(render(&state, 80, 14).contains("END\""));
}

mod collections {
    use curst_core::model::EnvId;

    use super::*;
    use crate::app::sample;
    use crate::app::{Focus, NodeKey, Popup};

    fn with_collection() -> State {
        let mut state = State::default();
        let mut workspace = sample();

        workspace.active_environment = EnvId::new("e1");
        update(
            &mut state,
            Action::Loaded {
                collections: vec![workspace],
                globals: vec![],
                problems: vec![],
            },
        );
        state.focus = Focus::Sidebar;

        state
    }

    fn key(path: &[&str]) -> NodeKey {
        NodeKey {
            collection: curst_core::model::LocalId::new("c1").unwrap(),
            path: path.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    #[test]
    fn sidebar_with_an_open_changed_request() {
        let mut state = with_collection();

        state.sidebar.expanded.insert(key(&["users"]));
        state.sidebar.selected = Some(key(&["create"]));
        update(&mut state, Action::Edit);
        state.url.insert_str("http://localhost/pets");
        state.sidebar.cut = Some(key(&["users", "list"]));

        insta::assert_snapshot!(render(&state, 100, 12));
        assert!(hints(&state).contains(&("Ctrl+S", "save")));
    }

    #[test]
    fn environment_picker() {
        let mut state = with_collection();

        update(&mut state, Action::Environments);

        insta::assert_snapshot!(render(&state, 80, 12));
        assert!(hints(&state).contains(&("v", "variables")));
    }

    #[test]
    fn variable_editor_and_confirm() {
        let mut state = with_collection();

        update(&mut state, Action::Environments);
        update(&mut state, Action::Down);
        update(&mut state, Action::EditVariables);

        insta::assert_snapshot!("variable_editor", render(&state, 80, 12));

        update(&mut state, Action::DeleteRow);
        update(&mut state, Action::ClosePopup);

        assert!(matches!(state.popup, Some(Popup::Confirm { .. })));
        insta::assert_snapshot!("confirm", render(&state, 80, 12));
    }

    #[test]
    fn names_from_files_cannot_reach_the_terminal() {
        let mut state = with_collection();

        state.collections[0].collection.name = "evil\u{1b}]0;owned\u{7}".into();

        let screen = render(&state, 80, 6);

        assert!(!screen.contains('\u{1b}'));
        assert!(!screen.contains('\u{7}'));
        assert!(screen.contains("evil␛]0;owned␇"));
    }
}
