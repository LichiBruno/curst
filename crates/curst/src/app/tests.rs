use std::time::Duration;

use curst_core::http::{HttpError, Response, ResponseBody, Timings};
use curst_core::model::{Auth, Body, KeyValue, Method};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

use super::*;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// feeds a key through the key map and `update`, like the event loop does
fn press(state: &mut State, code: KeyCode) -> Vec<Command> {
    press_key(state, key(code))
}

fn press_key(state: &mut State, event: KeyEvent) -> Vec<Command> {
    match action_for(state, event) {
        Some(action) => update(state, action),
        None => Vec::new(),
    }
}

fn type_text(state: &mut State, text: &str) {
    for c in text.chars() {
        press(state, KeyCode::Char(c));
    }
}

pub(crate) fn response(body: &str) -> Response {
    Response {
        status: 200,
        status_line: "HTTP/1.1 200 OK".into(),
        headers: vec![("content-type".into(), "application/json".into())],
        body: ResponseBody::from_bytes(body.as_bytes().to_vec()),
        timings: Timings {
            dns: Duration::from_millis(1),
            connect: Duration::from_millis(2),
            tls: Duration::ZERO,
            first_byte: Duration::from_millis(30),
            total: Duration::from_millis(42),
        },
        effective_url: "http://localhost/".into(),
        redirects: 0,
    }
}

fn sent(commands: &[Command]) -> (u64, &curst_core::model::Request) {
    match commands.last() {
        Some(Command::Send { id, request }) => (*id, request),
        other => panic!("expected a send, got {other:?}"),
    }
}

#[test]
fn quit_keys() {
    let mut state = State::default();

    press_key(&mut state, ctrl('c'));

    assert!(state.should_quit);

    let mut state = State {
        focus: Focus::Response,
        ..State::default()
    };
    press(&mut state, KeyCode::Char('q'));

    assert!(state.should_quit);
}

#[test]
fn q_is_typed_into_the_url() {
    let mut state = State::default();

    type_text(&mut state, "q");

    assert!(!state.should_quit);
    assert_eq!(state.request().url, "q");
}

#[test]
fn key_releases_are_ignored() {
    let state = State::default();
    let release = KeyEvent {
        kind: KeyEventKind::Release,
        state: KeyEventState::NONE,
        ..ctrl('c')
    };

    assert!(action_for(&state, release).is_none());
}

#[test]
fn enter_in_the_url_sends_the_request() {
    let mut state = State::default();

    type_text(&mut state, " http://localhost/x ");

    let commands = press(&mut state, KeyCode::Enter);
    let (id, request) = sent(&commands);

    assert_eq!(request.url, "http://localhost/x");
    assert_eq!(request.method, Method::Get);
    assert_eq!(state.in_flight, Some(id));
}

#[test]
fn sending_without_a_url_asks_for_one() {
    let mut state = State {
        focus: Focus::Response,
        ..State::default()
    };

    let commands = press_key(&mut state, ctrl('r'));

    assert_eq!(commands, Vec::new());
    assert_eq!(state.focus, Focus::Url);
    assert_eq!(state.notice.as_deref(), Some("type a URL first"));
}

#[test]
fn method_changes_with_arrows() {
    let mut state = State::default();

    press(&mut state, KeyCode::BackTab);

    assert_eq!(state.focus, Focus::Method);

    press(&mut state, KeyCode::Right);

    assert_eq!(state.method, Method::Post);

    press(&mut state, KeyCode::Left);
    press(&mut state, KeyCode::Left);

    assert_eq!(state.method, Method::Options);
}

#[test]
fn focus_cycles_both_ways() {
    let mut state = State::default();

    for expected in [
        Focus::Request,
        Focus::Response,
        Focus::Sidebar,
        Focus::Method,
        Focus::Url,
    ] {
        press(&mut state, KeyCode::Tab);

        assert_eq!(state.focus, expected);
    }

    press(&mut state, KeyCode::BackTab);

    assert_eq!(state.focus, Focus::Method);

    press(&mut state, KeyCode::BackTab);

    assert_eq!(state.focus, Focus::Sidebar);
}

#[test]
fn headers_and_params_are_edited_as_rows() {
    let mut state = State::default();

    type_text(&mut state, "http://h/");
    press(&mut state, KeyCode::Tab);
    press(&mut state, KeyCode::Char('a'));
    type_text(&mut state, "q");
    press(&mut state, KeyCode::Tab);
    type_text(&mut state, "rust");
    press(&mut state, KeyCode::Enter);

    press(&mut state, KeyCode::Right);

    assert_eq!(state.request_tab, RequestTab::Headers);

    press(&mut state, KeyCode::Enter);
    type_text(&mut state, "X-Off");
    press(&mut state, KeyCode::Tab);
    type_text(&mut state, "1");
    press(&mut state, KeyCode::Esc);
    press(&mut state, KeyCode::Char(' '));

    let commands = press_key(&mut state, ctrl('r'));
    let (_, request) = sent(&commands);

    assert_eq!(request.params, vec![KeyValue::new("q", "rust")]);
    assert_eq!(
        request.headers,
        vec![KeyValue {
            name: "X-Off".into(),
            value: "1".into(),
            enabled: false
        }]
    );
    assert_eq!(request.full_url(), "http://h/?q=rust");
}

#[test]
fn rows_can_be_deleted() {
    let mut state = State {
        focus: Focus::Request,
        ..State::default()
    };
    press(&mut state, KeyCode::Char('a'));
    type_text(&mut state, "a");
    press(&mut state, KeyCode::Enter);
    press(&mut state, KeyCode::Char('d'));

    assert_eq!(state.params.rows, Vec::new());
}

#[test]
fn json_body_is_multi_line() {
    let mut state = State {
        focus: Focus::Request,
        request_tab: RequestTab::Body,
        ..State::default()
    };
    press(&mut state, KeyCode::Enter);

    assert_eq!(
        state.notice.as_deref(),
        Some("choose a body type first, press t")
    );

    press(&mut state, KeyCode::Char('t'));
    press(&mut state, KeyCode::Enter);
    type_text(&mut state, "{");
    press(&mut state, KeyCode::Enter);
    type_text(&mut state, "}");

    assert!(state.request_typing());

    press(&mut state, KeyCode::Esc);

    assert!(!state.request_typing());
    assert_eq!(state.request().body, Body::Json("{\n}".into()));
}

#[test]
fn form_body_uses_rows() {
    let mut state = State {
        focus: Focus::Request,
        request_tab: RequestTab::Body,
        ..State::default()
    };

    for _ in 0..3 {
        press(&mut state, KeyCode::Char('t'));
    }

    press(&mut state, KeyCode::Char('a'));
    type_text(&mut state, "a");
    press(&mut state, KeyCode::Tab);
    type_text(&mut state, "1");
    press(&mut state, KeyCode::Enter);

    assert_eq!(
        state.request().body,
        Body::Form(vec![KeyValue::new("a", "1")])
    );
}

#[test]
fn bearer_and_basic_auth() {
    let mut state = State {
        focus: Focus::Request,
        request_tab: RequestTab::Auth,
        ..State::default()
    };
    press(&mut state, KeyCode::Enter);

    assert!(!state.auth.typing);

    press(&mut state, KeyCode::Char('t'));
    press(&mut state, KeyCode::Enter);
    type_text(&mut state, "me");
    press(&mut state, KeyCode::Tab);
    type_text(&mut state, "pw");
    press(&mut state, KeyCode::Enter);

    assert_eq!(
        state.request().auth,
        Auth::Basic {
            username: "me".into(),
            password: "pw".into()
        }
    );

    press(&mut state, KeyCode::Char('t'));
    press(&mut state, KeyCode::Down);
    press(&mut state, KeyCode::Up);
    press(&mut state, KeyCode::Enter);
    type_text(&mut state, "t0k");
    press(&mut state, KeyCode::Esc);

    assert_eq!(
        state.request().auth,
        Auth::Bearer {
            token: "t0k".into()
        }
    );
}

#[test]
fn response_arrives_and_is_shown() {
    let mut state = State::default();

    type_text(&mut state, "http://h/");

    let (id, _) = sent(&press(&mut state, KeyCode::Enter)).to_owned();

    update(
        &mut state,
        Action::responded(id, Ok(response(r#"{"a":1}"#))),
    );

    assert_eq!(state.in_flight, None);

    let Some(Outcome::Response(view)) = &state.outcome else {
        panic!("no response");
    };

    assert_eq!(view.lines(ResponseTab::Body).len(), 3);
}

#[test]
fn errors_are_shown() {
    let mut state = State::default();

    type_text(&mut state, "http://h/");

    let (id, _) = sent(&press(&mut state, KeyCode::Enter));

    update(
        &mut state,
        Action::responded(id, Err(HttpError::Connect("refused".into()))),
    );

    assert!(matches!(&state.outcome, Some(Outcome::Error(m)) if m == "could not connect: refused"));
}

#[test]
fn stale_and_cancelled_results_are_dropped() {
    let mut state = State::default();

    type_text(&mut state, "http://h/");

    let (first, _) = sent(&press(&mut state, KeyCode::Enter));
    let commands = press_key(&mut state, ctrl('r'));

    assert_eq!(commands.first(), Some(&Command::Cancel));

    let (second, _) = sent(&commands);

    assert_ne!(first, second);

    update(&mut state, Action::responded(first, Ok(response("old"))));

    assert!(state.outcome.is_none());
    assert_eq!(state.in_flight, Some(second));

    assert_eq!(press(&mut state, KeyCode::Esc), vec![Command::Cancel]);
    assert_eq!(state.notice.as_deref(), Some("request cancelled"));

    update(
        &mut state,
        Action::responded(second, Err(HttpError::Cancelled)),
    );

    assert!(state.outcome.is_none());
    assert_eq!(press(&mut state, KeyCode::Esc), Vec::new());
}

fn with_response(body: &str) -> State {
    let mut state = State::default();

    type_text(&mut state, "http://h/");

    let (id, _) = sent(&press(&mut state, KeyCode::Enter));

    update(&mut state, Action::responded(id, Ok(response(body))));
    state.focus = Focus::Response;

    state
}

#[test]
fn response_scrolls_within_bounds() {
    let body = format!("[{}]", vec!["1"; 40].join(","));
    let mut state = with_response(&body);

    press(&mut state, KeyCode::Down);

    assert_eq!(state.scroll, 1);

    press(&mut state, KeyCode::PageDown);

    assert_eq!(state.scroll, 11);

    press(&mut state, KeyCode::End);

    assert_eq!(state.scroll, 32);

    press(&mut state, KeyCode::PageDown);

    assert_eq!(state.scroll, 32);

    press(&mut state, KeyCode::Up);

    assert_eq!(state.scroll, 31);

    press(&mut state, KeyCode::PageUp);

    assert_eq!(state.scroll, 21);

    state.response_height.set(30);
    press(&mut state, KeyCode::End);

    assert_eq!(state.scroll, 12);

    press(&mut state, KeyCode::PageUp);

    assert_eq!(state.scroll, 0);

    press(&mut state, KeyCode::Home);

    assert_eq!(state.scroll, 0);

    press(&mut state, KeyCode::Down);
    press(&mut state, KeyCode::Right);

    assert_eq!(state.response_tab, ResponseTab::Headers);
    assert_eq!(state.scroll, 0);

    press(&mut state, KeyCode::Left);
    press(&mut state, KeyCode::Left);

    assert_eq!(state.response_tab, ResponseTab::Raw);
}

#[test]
fn save_prompt_flow() {
    let mut state = State {
        focus: Focus::Response,
        ..State::default()
    };
    press(&mut state, KeyCode::Char('s'));

    assert_eq!(state.notice.as_deref(), Some("nothing to save yet"));

    let mut state = with_response("{}");

    press(&mut state, KeyCode::Char('s'));

    let Some(Popup::Prompt { input, .. }) = &state.popup else {
        panic!("no prompt");
    };

    assert_eq!(editor::text_of(input), "response.json");

    press(&mut state, KeyCode::Char('q'));

    assert!(!state.should_quit);
    assert_eq!(
        press(&mut state, KeyCode::Enter),
        vec![Command::SaveBody("response.jsonq".into())]
    );

    update(&mut state, Action::Saved(Err("exists".into())));

    assert!(state.popup.is_some());
    assert_eq!(state.notice.as_deref(), Some("exists"));

    update(&mut state, Action::Saved(Ok("out.json".into())));

    assert!(state.popup.is_none());
    assert_eq!(state.notice.as_deref(), Some("saved to out.json"));

    press(&mut state, KeyCode::Char('s'));
    press(&mut state, KeyCode::Esc);

    assert!(state.popup.is_none());
}

#[test]
fn text_body_has_an_editable_content_type() {
    let mut state = State {
        focus: Focus::Request,
        request_tab: RequestTab::Body,
        ..State::default()
    };

    press(&mut state, KeyCode::Char('c'));

    assert!(!state.request_typing());

    press(&mut state, KeyCode::Char('t'));
    press(&mut state, KeyCode::Char('t'));
    press(&mut state, KeyCode::Char('c'));

    assert_eq!(state.body.typing, Some(BodyField::ContentType));

    for _ in 0.."text/plain".len() {
        press(&mut state, KeyCode::Backspace);
    }

    type_text(&mut state, "application/xml");
    press(&mut state, KeyCode::Enter);

    assert!(!state.request_typing());

    press(&mut state, KeyCode::Char('c'));
    press(&mut state, KeyCode::Tab);

    assert_eq!(state.body.typing, Some(BodyField::Text));

    type_text(&mut state, "<a/>");
    press(&mut state, KeyCode::Enter);
    press(&mut state, KeyCode::Esc);

    assert_eq!(
        state.request().body,
        Body::Text {
            content_type: "application/xml".into(),
            text: "<a/>\n".into(),
        }
    );
}

#[test]
fn response_scrolls_sideways_within_the_longest_line() {
    let mut state = with_response(&format!("[\"{}\"]", "x".repeat(100)));

    // the longest line is the string with its indent and quotes
    assert_eq!(state.last_column(), 104);

    state.response_width.set(40);

    press(&mut state, KeyCode::Char('l'));

    assert_eq!(state.column, 8);

    press_key(
        &mut state,
        KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT),
    );

    assert_eq!(state.column, 16);
    assert_eq!(state.response_tab, ResponseTab::Body);

    for _ in 0..20 {
        press(&mut state, KeyCode::Char('l'));
    }

    assert_eq!(state.column, 64);

    press(&mut state, KeyCode::Char('h'));
    press_key(
        &mut state,
        KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT),
    );

    assert_eq!(state.column, 48);

    press(&mut state, KeyCode::Right);

    assert_eq!(state.response_tab, ResponseTab::Headers);
    assert_eq!(state.column, 0);
}

mod collections {
    use curst_core::model::{EnvId, Item, Variable};
    use curst_core::vars::VarName;

    use super::*;
    use crate::app::workspace::tests::sample;

    fn node(path: &[&str]) -> NodeKey {
        NodeKey {
            collection: LocalId::new("c1").unwrap(),
            path: path.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    fn with_collection() -> State {
        let mut state = State::default();

        update(
            &mut state,
            Action::Loaded {
                collections: vec![sample()],
                globals: vec![],
                problems: vec![],
            },
        );
        state.focus = Focus::Sidebar;

        state
    }

    fn shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    /// the reply the runtime sends after a change, with the collection as
    /// it was
    fn stored(state: &mut State, stored: Stored) {
        update(
            state,
            Action::Stored {
                collection: LocalId::new("c1").unwrap(),
                result: Ok(Box::new(stored)),
            },
        );
    }

    fn open_create(state: &mut State) {
        state.sidebar.selected = Some(node(&["create"]));
        press(state, KeyCode::Enter);
    }

    #[test]
    fn loading_selects_and_expands_the_collections() {
        let state = with_collection();

        assert_eq!(state.sidebar.selected, Some(node(&[])));
        assert!(state.sidebar.expanded.contains(&node(&[])));
        assert_eq!(state.current_workspace().map(|w| w.id.as_str()), Some("c1"));

        let mut state = State::default();

        update(
            &mut state,
            Action::Loaded {
                collections: vec![],
                globals: vec![],
                problems: vec!["a.toml: bad".into(), "b.toml: bad".into()],
            },
        );

        assert_eq!(
            state.notice.as_deref(),
            Some("skipped a.toml: bad and 1 more")
        );
    }

    #[test]
    fn the_tree_is_walked_and_requests_open() {
        let mut state = with_collection();

        press(&mut state, KeyCode::Down);

        assert_eq!(state.sidebar.selected, Some(node(&["users"])));

        press(&mut state, KeyCode::Right);
        press(&mut state, KeyCode::Down);

        assert_eq!(state.sidebar.selected, Some(node(&["users", "list"])));

        press(&mut state, KeyCode::Left);

        assert_eq!(state.sidebar.selected, Some(node(&["users"])));

        press(&mut state, KeyCode::Left);

        assert!(!state.sidebar.expanded.contains(&node(&["users"])));

        press(&mut state, KeyCode::Down);
        press(&mut state, KeyCode::Enter);

        assert_eq!(state.open.as_ref().map(|o| o.name.as_str()), Some("CREATE"));
        assert_eq!(state.method, Method::Post);
        assert!(!state.is_dirty());

        press(&mut state, KeyCode::End);
        press(&mut state, KeyCode::Home);

        assert_eq!(state.sidebar.selected, Some(node(&[])));

        press(&mut state, KeyCode::Enter);

        assert!(!state.sidebar.expanded.contains(&node(&[])));
    }

    #[test]
    fn unsaved_changes_are_never_lost_silently() {
        let mut state = with_collection();

        open_create(&mut state);
        state.focus = Focus::Url;
        type_text(&mut state, "http://changed/");

        assert!(state.is_dirty());

        state.focus = Focus::Sidebar;
        state.sidebar.selected = Some(node(&["users"]));
        press(&mut state, KeyCode::Right);
        press(&mut state, KeyCode::Down);
        press(&mut state, KeyCode::Enter);

        assert!(matches!(state.popup, Some(Popup::Confirm { .. })));

        press(&mut state, KeyCode::Char('n'));

        assert!(state.popup.is_none());
        assert_eq!(state.open.as_ref().map(|o| o.name.as_str()), Some("CREATE"));

        press(&mut state, KeyCode::Char('q'));

        assert!(!state.should_quit);
        assert!(matches!(state.popup, Some(Popup::Confirm { .. })));

        press(&mut state, KeyCode::Esc);
        press(&mut state, KeyCode::Enter);
        press(&mut state, KeyCode::Char('y'));

        assert_eq!(state.open.as_ref().map(|o| o.name.as_str()), Some("LIST"));
        assert!(!state.is_dirty());
    }

    #[test]
    fn ctrl_c_twice_quits_with_unsaved_changes() {
        let mut state = with_collection();

        open_create(&mut state);
        state.focus = Focus::Url;
        type_text(&mut state, "x");
        press_key(&mut state, ctrl('c'));

        assert!(!state.should_quit);

        press_key(&mut state, ctrl('c'));

        assert!(state.should_quit);
    }

    #[test]
    fn saving_an_open_request() {
        let mut state = with_collection();

        open_create(&mut state);
        state.focus = Focus::Url;
        type_text(&mut state, "http://h/");

        let commands = press_key(&mut state, ctrl('s'));

        assert_eq!(
            commands,
            [Command::Store {
                collection: LocalId::new("c1").unwrap(),
                change: Change::SaveRequest {
                    path: vec!["create".into()],
                    request: state.request(),
                },
            }]
        );

        let written = state.request();

        // a key typed before the reply arrives is not part of the save
        type_text(&mut state, "x");
        stored(
            &mut state,
            Stored {
                workspace: Some(sample()),
                saved: Some((node(&["create"]), written)),
                ..Stored::default()
            },
        );

        assert!(state.is_dirty());
        assert_eq!(state.notice.as_deref(), Some("saved"));

        press(&mut state, KeyCode::Backspace);

        assert!(!state.is_dirty());
    }

    #[test]
    fn saving_a_new_request_asks_for_a_name() {
        let mut state = State::default();

        type_text(&mut state, "http://h/pets/{{id}}?x=1");
        press_key(&mut state, ctrl('s'));

        assert_eq!(
            state.notice.as_deref(),
            Some("create a collection first: press c in the sidebar")
        );

        let mut state = with_collection();

        state.focus = Focus::Url;
        type_text(&mut state, "http://h/pets/{{id}}?x=1");
        press_key(&mut state, ctrl('s'));

        let Some(Popup::Prompt { input, .. }) = &state.popup else {
            panic!("no prompt");
        };

        assert_eq!(editor::text_of(input), "pets");

        let commands = press(&mut state, KeyCode::Enter);
        let [Command::Store { change, .. }] = commands.as_slice() else {
            panic!("no store");
        };

        assert!(matches!(
            change,
            Change::CreateRequest { open: true, parent, .. } if parent.is_empty()
        ));

        let written = state.request();

        stored(
            &mut state,
            Stored {
                workspace: Some(sample()),
                created: Some(node(&["create"])),
                saved: Some((node(&["create"]), written)),
                ..Stored::default()
            },
        );

        assert_eq!(state.open.as_ref().map(|o| o.name.as_str()), Some("CREATE"));
        assert_eq!(state.sidebar.selected, Some(node(&["create"])));
        assert!(!state.is_dirty());
    }

    #[test]
    fn variables_are_resolved_before_sending() {
        let mut state = with_collection();
        let var = |name: &str, value: &str| Variable::new(VarName::new(name).unwrap(), value);

        state.collections[0].active_environment = EnvId::new("e1");
        state.collections[0].collection.variables = vec![var("host", "h{{a}}")];
        state.globals = vec![var("path", "p")];
        state.focus = Focus::Url;
        type_text(&mut state, "http://{{host}}/{{path}}");

        let commands = press(&mut state, KeyCode::Enter);

        assert_eq!(sent(&commands).1.url, "http://h1/p");

        type_text(&mut state, "/{{gone}}/{{also}}");

        assert_eq!(press(&mut state, KeyCode::Enter), Vec::new());
        assert_eq!(
            state.notice.as_deref(),
            Some("not sent: {{also}}, {{gone}} not defined in Dev")
        );

        state.globals = vec![var("gone", "{{also}}"), var("also", "{{gone}}")];

        assert_eq!(press(&mut state, KeyCode::Enter), Vec::new());
        assert!(
            state
                .notice
                .as_deref()
                .unwrap()
                .contains("refer to themselves")
        );
    }

    #[test]
    fn sidebar_keys_change_the_collection() {
        let mut state = with_collection();
        let c1 = LocalId::new("c1").unwrap();
        let store = |change| Command::Store {
            collection: c1.clone(),
            change,
        };

        state.sidebar.selected = Some(node(&["create"]));
        press(&mut state, KeyCode::Char('n'));
        type_text(&mut state, "Get pet");

        assert_eq!(
            press(&mut state, KeyCode::Enter),
            [store(Change::CreateRequest {
                parent: vec![],
                name: "Get pet".into(),
                request: Request::default(),
                open: false,
            })]
        );

        state.sidebar.selected = Some(node(&["users"]));
        press(&mut state, KeyCode::Char('f'));
        type_text(&mut state, "Admin");

        assert_eq!(
            press(&mut state, KeyCode::Enter),
            [store(Change::CreateFolder {
                parent: vec!["users".into()],
                name: "Admin".into(),
            })]
        );

        press(&mut state, KeyCode::Char('r'));
        type_text(&mut state, "s");

        assert_eq!(
            press(&mut state, KeyCode::Enter),
            [store(Change::Rename {
                path: vec!["users".into()],
                name: "Userss".into(),
            })]
        );

        press(&mut state, KeyCode::Char('d'));

        assert_eq!(
            press(&mut state, KeyCode::Char('y')),
            [store(Change::Delete {
                path: vec!["users".into()],
            })]
        );

        state.sidebar.selected = Some(node(&["create"]));
        press(&mut state, KeyCode::Char('x'));

        assert_eq!(state.sidebar.cut, Some(node(&["create"])));

        state.sidebar.selected = Some(node(&["users"]));

        assert_eq!(
            press(&mut state, KeyCode::Char('p')),
            [store(Change::Move {
                path: vec!["create".into()],
                parent: vec!["users".into()],
            })]
        );
        assert_eq!(press(&mut state, KeyCode::Char('p')), Vec::new());

        state.sidebar.selected = Some(node(&["users"]));

        assert_eq!(
            press_key(&mut state, shift(KeyCode::Down)),
            [store(Change::Reorder {
                path: vec!["users".into()],
                index: 1,
            })]
        );
        assert_eq!(press_key(&mut state, shift(KeyCode::Up)), Vec::new());

        state.sidebar.selected = Some(node(&[]));
        press(&mut state, KeyCode::Char('r'));
        type_text(&mut state, "!");

        assert_eq!(
            press(&mut state, KeyCode::Enter),
            [store(Change::RenameCollection("Pets!".into()))]
        );

        press(&mut state, KeyCode::Char('d'));

        assert_eq!(
            press(&mut state, KeyCode::Enter),
            [store(Change::DeleteCollection)]
        );

        press(&mut state, KeyCode::Char('c'));
        type_text(&mut state, "New");

        assert_eq!(
            press(&mut state, KeyCode::Enter),
            [Command::CreateCollection("New".into())]
        );
    }

    #[test]
    fn moving_needs_a_sensible_target() {
        let mut state = with_collection();

        state.sidebar.selected = Some(node(&[]));
        press(&mut state, KeyCode::Char('x'));

        assert_eq!(state.sidebar.cut, None);

        press(&mut state, KeyCode::Char('p'));

        assert!(state.notice.as_deref().unwrap().starts_with("nothing cut"));

        state.sidebar.selected = Some(node(&["users"]));
        press(&mut state, KeyCode::Char('x'));
        state.sidebar.expanded.insert(node(&["users"]));
        state.sidebar.selected = Some(node(&["users", "list"]));

        assert_eq!(press(&mut state, KeyCode::Char('p')), Vec::new());
        assert_eq!(
            state.notice.as_deref(),
            Some("a folder cannot move into itself")
        );
    }

    #[test]
    fn renames_moves_and_deletes_follow_the_open_request() {
        let mut state = with_collection();

        open_create(&mut state);

        let mut moved = sample();

        moved.collection.items.retain(|i| i.slug() != "create");

        let Some(Item::Folder(users)) = moved.collection.items.first_mut() else {
            panic!("no folder");
        };

        let Some(Item::Request(mut create)) = sample().collection.item(&["create".into()]).cloned()
        else {
            panic!("no request");
        };

        create.slug = "added".into();
        create.name = "Added".into();
        users.items.push(Item::Request(create));

        stored(
            &mut state,
            Stored {
                workspace: Some(moved),
                moved: Some((node(&["create"]), node(&["users", "added"]))),
                ..Stored::default()
            },
        );

        let open = state.open.as_ref().unwrap();

        assert_eq!(
            (open.key.clone(), open.name.as_str()),
            (node(&["users", "added"]), "Added")
        );
        assert_eq!(state.sidebar.selected, Some(node(&["users", "added"])));

        // deleting it keeps what the editor holds, as an unsaved request
        stored(
            &mut state,
            Stored {
                workspace: Some(sample()),
                ..Stored::default()
            },
        );

        assert!(state.open.is_none());
        assert_eq!(state.method, Method::Post);
        assert_eq!(state.sidebar.selected, Some(node(&["users"])));

        // a deleted collection takes its keys along
        stored(&mut state, Stored::default());

        assert_eq!(state.collections, Vec::new());
        assert_eq!(state.sidebar.selected, None);

        update(
            &mut state,
            Action::Stored {
                collection: LocalId::new("c1").unwrap(),
                result: Err("disk full".into()),
            },
        );

        assert_eq!(state.notice.as_deref(), Some("disk full"));
    }

    #[test]
    fn choosing_and_managing_environments() {
        let mut state = State::default();

        press_key(&mut state, ctrl('e'));

        assert!(state.popup.is_none());
        assert!(state.notice.is_some());

        let mut state = with_collection();

        press_key(&mut state, ctrl('e'));

        assert!(matches!(
            state.popup,
            Some(Popup::Environments { selected: 0, .. })
        ));

        press(&mut state, KeyCode::Down);
        press(&mut state, KeyCode::Down);

        assert_eq!(
            press(&mut state, KeyCode::Enter),
            [Command::SetActiveEnvironment {
                collection: LocalId::new("c1").unwrap(),
                environment: EnvId::new("e1"),
            }]
        );
        assert_eq!(state.collections[0].environment_name(), Some("Dev"));

        press_key(&mut state, ctrl('e'));

        assert!(matches!(
            state.popup,
            Some(Popup::Environments { selected: 1, .. })
        ));

        press(&mut state, KeyCode::Char('r'));
        type_text(&mut state, "2");

        assert_eq!(
            press(&mut state, KeyCode::Enter),
            [Command::Store {
                collection: LocalId::new("c1").unwrap(),
                change: Change::RenameEnvironment {
                    id: EnvId::new("e1").unwrap(),
                    name: "Dev2".into(),
                },
            }]
        );

        press_key(&mut state, ctrl('e'));
        press(&mut state, KeyCode::Char('d'));

        assert_eq!(
            press(&mut state, KeyCode::Char('y')),
            [Command::Store {
                collection: LocalId::new("c1").unwrap(),
                change: Change::DeleteEnvironment(EnvId::new("e1").unwrap()),
            }]
        );

        press_key(&mut state, ctrl('e'));
        press(&mut state, KeyCode::Char('n'));
        type_text(&mut state, "Prod");

        assert_eq!(
            press(&mut state, KeyCode::Enter),
            [Command::Store {
                collection: LocalId::new("c1").unwrap(),
                change: Change::CreateEnvironment("Prod".into()),
            }]
        );

        press_key(&mut state, ctrl('e'));
        press(&mut state, KeyCode::Up);
        press(&mut state, KeyCode::Enter);

        assert_eq!(state.collections[0].active_environment, None);

        press_key(&mut state, ctrl('e'));
        press(&mut state, KeyCode::Esc);

        assert!(state.popup.is_none());
    }

    #[test]
    fn the_variable_editor_checks_names_before_saving() {
        let mut state = with_collection();

        state.collections[0].active_environment = EnvId::new("e1");
        press_key(&mut state, ctrl('e'));
        press(&mut state, KeyCode::Char('v'));

        let Some(Popup::Variables(editor)) = &state.popup else {
            panic!("no editor");
        };

        assert_eq!(editor.tab, VarTab::Environment);
        assert_eq!(editor.table().rows.len(), 1);

        // a second `a` in the environment, then a bad name in the collection
        press(&mut state, KeyCode::Char('a'));
        type_text(&mut state, "a");
        press(&mut state, KeyCode::Tab);
        type_text(&mut state, "2");
        press(&mut state, KeyCode::Enter);
        press(&mut state, KeyCode::Right);
        press(&mut state, KeyCode::Char('a'));
        type_text(&mut state, "bad name");
        press(&mut state, KeyCode::Enter);

        assert_eq!(press_key(&mut state, ctrl('s')), Vec::new());
        assert_eq!(state.notice.as_deref(), Some("a is defined twice"));

        let Some(Popup::Variables(editor)) = &state.popup else {
            panic!("editor closed");
        };

        assert_eq!(
            (editor.tab, editor.table().selected),
            (VarTab::Environment, 1)
        );

        press(&mut state, KeyCode::Char('d'));

        assert_eq!(press_key(&mut state, ctrl('s')), Vec::new());
        assert!(
            state
                .notice
                .as_deref()
                .unwrap()
                .contains("not a valid name")
        );

        press(&mut state, KeyCode::Enter);

        for _ in 0.."bad name".len() {
            press(&mut state, KeyCode::Backspace);
        }

        type_text(&mut state, "good");
        press(&mut state, KeyCode::Enter);
        press(&mut state, KeyCode::Right);
        press(&mut state, KeyCode::Char('a'));
        type_text(&mut state, "g");
        press(&mut state, KeyCode::Enter);
        press(&mut state, KeyCode::Char(' '));

        let commands = press_key(&mut state, ctrl('s'));
        let good = Variable::new(VarName::new("good").unwrap(), "");
        let mut g = Variable::new(VarName::new("g").unwrap(), "");

        g.enabled = false;

        assert_eq!(
            commands,
            [
                Command::Store {
                    collection: LocalId::new("c1").unwrap(),
                    change: Change::SaveVariables(vec![good]),
                },
                Command::SaveGlobals(vec![g.clone()]),
            ]
        );
        assert!(state.popup.is_none());

        update(&mut state, Action::GlobalsSaved(Ok(vec![g.clone()])));

        assert_eq!(state.globals, [g]);
    }

    #[test]
    fn ctrl_s_saves_while_a_variable_is_being_typed() {
        let mut state = with_collection();

        press_key(&mut state, ctrl('e'));
        press(&mut state, KeyCode::Char('v'));
        press(&mut state, KeyCode::Char('a'));
        type_text(&mut state, "token");
        press(&mut state, KeyCode::Tab);
        type_text(&mut state, "abc");

        assert_eq!(
            press_key(&mut state, ctrl('s')),
            [Command::Store {
                collection: LocalId::new("c1").unwrap(),
                change: Change::SaveVariables(vec![Variable::new(
                    VarName::new("token").unwrap(),
                    "abc"
                )]),
            }]
        );
    }

    #[test]
    fn quitting_with_unsaved_variables_asks_and_can_go_back() {
        let mut state = with_collection();

        press_key(&mut state, ctrl('e'));
        press(&mut state, KeyCode::Char('v'));
        press(&mut state, KeyCode::Char('a'));
        type_text(&mut state, "x");
        press(&mut state, KeyCode::Enter);
        press_key(&mut state, ctrl('c'));

        assert!(!state.should_quit);
        assert!(matches!(state.popup, Some(Popup::Confirm { .. })));

        press(&mut state, KeyCode::Char('n'));

        assert!(matches!(state.popup, Some(Popup::Variables(_))));

        press_key(&mut state, ctrl('c'));
        press_key(&mut state, ctrl('c'));

        assert!(state.should_quit);
    }

    #[test]
    fn closing_the_variable_editor_with_changes_asks_first() {
        let mut state = with_collection();

        press_key(&mut state, ctrl('e'));
        press(&mut state, KeyCode::Char('v'));

        let Some(Popup::Variables(editor)) = &state.popup else {
            panic!("no editor");
        };

        assert_eq!(editor.tabs(), [VarTab::Collection, VarTab::Globals]);

        press(&mut state, KeyCode::Esc);

        assert!(state.popup.is_none());

        press_key(&mut state, ctrl('e'));
        press(&mut state, KeyCode::Char('v'));
        press(&mut state, KeyCode::Enter);
        type_text(&mut state, "x");
        press(&mut state, KeyCode::Esc);
        press(&mut state, KeyCode::Esc);

        assert!(matches!(state.popup, Some(Popup::Confirm { .. })));

        press(&mut state, KeyCode::Char('n'));

        assert!(matches!(state.popup, Some(Popup::Variables(_))));

        press(&mut state, KeyCode::Esc);
        press(&mut state, KeyCode::Char('y'));

        assert!(state.popup.is_none());
    }

    #[test]
    fn new_collections_are_selected() {
        let mut state = State::default();
        let mut workspace = sample();

        workspace.collection.items.clear();
        update(&mut state, Action::CollectionAdded(Ok(Box::new(workspace))));

        assert_eq!(state.sidebar.selected, Some(node(&[])));

        update(&mut state, Action::CollectionAdded(Err("nope".into())));

        assert_eq!(state.notice.as_deref(), Some("nope"));
    }
}
