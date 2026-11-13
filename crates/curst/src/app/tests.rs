use std::time::Duration;

use curst_core::http::{HttpError, Response, ResponseBody, Timings};
use curst_core::model::{Auth, Body, KeyValue, Method};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

use super::*;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl_arrow(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// feeds a key through the key map and `update`, like the event loop does
fn press(state: &mut State, code: KeyCode) -> Vec<Command> {
    press_key(state, key(code))
}

fn press_key(state: &mut State, event: KeyEvent) -> Vec<Command> {
    action_for(state, event)
        .into_iter()
        .flat_map(|action| update(state, action))
        .collect()
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
        stopped: None,
    }
}

fn sent(commands: &[Command]) -> (u64, &curst_core::model::Request) {
    match commands.last() {
        Some(Command::Send { id, request, .. }) => (*id, request),
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

    assert_eq!(action_for(&state, release).len(), 0);
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
    press(&mut state, KeyCode::Esc);
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
fn stale_and_canceled_results_are_dropped() {
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
    assert_eq!(state.notice.as_deref(), Some("request canceled"));

    update(
        &mut state,
        Action::responded(second, Err(HttpError::Canceled)),
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
                orphans: vec![],
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
        // selecting a collection lends no variables: a tab belongs to one
        // only once it holds a request from it
        assert!(state.current_workspace().is_none());

        let mut state = State::default();

        update(
            &mut state,
            Action::Loaded {
                collections: vec![],
                globals: vec![],
                problems: vec!["a.toml: bad".into(), "b.toml: bad".into()],
                orphans: vec![],
            },
        );

        assert_eq!(state.notice.as_deref(), Some("a.toml: bad (and 1 more)"));
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

        // another request opens in a tab of its own, so nothing is lost
        state.focus = Focus::Sidebar;
        state.sidebar.selected = Some(node(&["users"]));
        press(&mut state, KeyCode::Right);
        press(&mut state, KeyCode::Down);
        press(&mut state, KeyCode::Enter);

        assert!(state.popup.is_none());
        assert_eq!(state.open.as_ref().map(|o| o.name.as_str()), Some("LIST"));
        assert_eq!(state.tab_count(), 2);
        assert!(state.any_dirty());

        // quitting asks while any tab has changes
        press(&mut state, KeyCode::Char('q'));

        assert!(!state.should_quit);
        assert!(matches!(state.popup, Some(Popup::Confirm { .. })));

        press(&mut state, KeyCode::Esc);

        // closing the changed tab asks too
        press_key(&mut state, ctrl_arrow(KeyCode::Left));

        assert_eq!(state.open.as_ref().map(|o| o.name.as_str()), Some("CREATE"));

        press_key(&mut state, ctrl('w'));

        assert!(matches!(state.popup, Some(Popup::Confirm { .. })));

        press(&mut state, KeyCode::Char('y'));

        assert_eq!(state.tab_count(), 1);
        assert_eq!(state.open.as_ref().map(|o| o.name.as_str()), Some("LIST"));
        assert!(!state.any_dirty());
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
    fn the_save_dialog_chooses_where_the_request_goes() {
        let mut state = with_collection();
        let mut untrusted = sample();

        untrusted.id = LocalId::new("c2").unwrap();
        untrusted.collection.name = "Strangers".into();
        untrusted.trusted = false;
        state.collections.push(untrusted);
        state.focus = Focus::Url;
        type_text(&mut state, "http://h/pets");
        press_key(&mut state, ctrl('s'));

        let Some(Popup::SaveRequest(dialog)) = &state.popup else {
            panic!("no save dialog");
        };
        let labels: Vec<&str> = dialog.places.iter().map(|(_, l)| l.as_str()).collect();

        assert_eq!(labels, ["Pets", "Pets › Users"]);

        // typing names the request; while the list is open it does not
        press(&mut state, KeyCode::Tab);
        press(&mut state, KeyCode::Char('x'));
        press(&mut state, KeyCode::Down);
        press(&mut state, KeyCode::Enter);

        let Some(Popup::SaveRequest(dialog)) = &state.popup else {
            panic!("picking a place closed the dialog");
        };

        assert!(!dialog.picking);
        assert_eq!(dialog.place, 1);
        assert_eq!(editor::text_of(&dialog.input), "pets");

        let commands = press(&mut state, KeyCode::Enter);
        let [Command::Store { collection, change }] = commands.as_slice() else {
            panic!("no store");
        };

        assert_eq!(collection.as_str(), "c1");
        assert!(matches!(
            change,
            Change::CreateRequest { parent, name, .. } if parent == &["users"] && name == "pets"
        ));
    }

    #[test]
    fn the_save_dialog_wants_a_name_and_esc_closes_the_list_first() {
        let mut state = with_collection();

        press_key(&mut state, ctrl('s'));

        if let Some(Popup::SaveRequest(dialog)) = &mut state.popup {
            *dialog.input = editor::single_line("  ", "name");
        }

        assert_eq!(press(&mut state, KeyCode::Enter), Vec::new());
        assert!(matches!(state.popup, Some(Popup::SaveRequest(_))));

        press(&mut state, KeyCode::Tab);
        press(&mut state, KeyCode::Esc);

        assert!(matches!(&state.popup, Some(Popup::SaveRequest(d)) if !d.picking));

        press(&mut state, KeyCode::Esc);

        assert!(state.popup.is_none());
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

        let Some(Popup::SaveRequest(dialog)) = &state.popup else {
            panic!("no save dialog");
        };

        assert_eq!(editor::text_of(&dialog.input), "pets");
        assert_eq!(dialog.places[dialog.place].1, "Pets");

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
        state.collection = LocalId::new("c1");
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

        state.globals = vec![Variable::secret(VarName::new("gone").unwrap())];

        assert_eq!(press(&mut state, KeyCode::Enter), Vec::new());
        assert_eq!(
            state.notice.as_deref(),
            Some(
                "not sent: {{also}}, {{path}} not defined in Dev; {{gone}} secret with no value yet; set it in the variable editor"
            )
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
        press(&mut state, KeyCode::Esc);

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
        press(&mut state, KeyCode::Esc);
        press(&mut state, KeyCode::Right);
        press(&mut state, KeyCode::Char('a'));
        type_text(&mut state, "g");
        press(&mut state, KeyCode::Esc);
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
    fn a_secret_variable_stays_secret_when_another_row_is_saved() {
        let mut state = with_collection();

        state.collections[0].collection.variables = vec![
            Variable::secret(VarName::new("token").unwrap()),
            Variable::new(VarName::new("host").unwrap(), "h"),
        ];
        press_key(&mut state, ctrl('e'));
        press(&mut state, KeyCode::Char('v'));
        press(&mut state, KeyCode::Down);
        press(&mut state, KeyCode::Enter);
        press(&mut state, KeyCode::Tab);
        type_text(&mut state, "2");
        press(&mut state, KeyCode::Enter);

        // the secret's value is typed blind, and Enter on nothing keeps it
        press(&mut state, KeyCode::Up);
        press(&mut state, KeyCode::Enter);
        press(&mut state, KeyCode::Tab);

        let Some(Popup::Variables(editor)) = &state.popup else {
            panic!("no editor");
        };

        assert!(editor.editing());

        press(&mut state, KeyCode::Enter);

        assert_eq!(
            press_key(&mut state, ctrl('s')),
            [Command::Store {
                collection: LocalId::new("c1").unwrap(),
                change: Change::SaveVariables(vec![
                    Variable::secret(VarName::new("token").unwrap()),
                    Variable::new(VarName::new("host").unwrap(), "h2"),
                ]),
            }]
        );
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

    /// the variable editor of the sample collection, with a row added
    fn with_changed_variables() -> State {
        let mut state = with_collection();

        press_key(&mut state, ctrl('e'));
        press(&mut state, KeyCode::Char('v'));
        press(&mut state, KeyCode::Char('a'));
        type_text(&mut state, "x");
        press(&mut state, KeyCode::Esc);

        state
    }

    fn asked(state: &State, quit: bool) -> bool {
        matches!(&state.popup, Some(Popup::SaveVariables { quit: q, .. }) if *q == quit)
    }

    #[test]
    fn quitting_with_unsaved_variables_offers_to_save_them() {
        let mut state = with_changed_variables();

        press_key(&mut state, ctrl('c'));

        assert!(!state.should_quit);
        assert!(asked(&state, true));

        // Esc goes back to them
        press(&mut state, KeyCode::Esc);

        assert!(matches!(state.popup, Some(Popup::Variables(_))));

        // n quits without them, and a second Ctrl+C does as well
        press_key(&mut state, ctrl('c'));
        press(&mut state, KeyCode::Char('n'));

        assert!(state.should_quit);

        let mut state = with_changed_variables();

        press_key(&mut state, ctrl('c'));
        press_key(&mut state, ctrl('c'));

        assert!(state.should_quit);

        // y saves them, then quits
        let mut state = with_changed_variables();

        press_key(&mut state, ctrl('c'));

        let commands = press(&mut state, KeyCode::Char('y'));

        assert!(matches!(
            commands.as_slice(),
            [Command::Store {
                change: Change::SaveVariables(_),
                ..
            }]
        ));
        assert!(state.should_quit);
    }

    #[test]
    fn quitting_after_saving_a_secret_waits_for_it() {
        let mut state = with_changed_variables();

        // the new row is made a secret with a value
        press(&mut state, KeyCode::Enter);
        press(&mut state, KeyCode::Tab);
        type_text(&mut state, "abc");
        press(&mut state, KeyCode::Esc);
        press(&mut state, KeyCode::Char('s'));
        press_key(&mut state, ctrl('c'));

        let commands = press(&mut state, KeyCode::Char('y'));
        let Some(Command::ChangeSecrets { id, .. }) = commands.first() else {
            panic!("{commands:?}");
        };

        // the secret is stored first, the file written after, then curst quits
        assert!(!state.should_quit);

        let commands = update(
            &mut state,
            Action::SecretsChanged {
                id: *id,
                result: Ok(()),
            },
        );

        assert!(matches!(commands.as_slice(), [Command::Store { .. }]));
        assert!(state.should_quit);
    }

    #[test]
    fn closing_changed_variables_offers_to_save_them() {
        let mut state = with_collection();

        press_key(&mut state, ctrl('e'));
        press(&mut state, KeyCode::Char('v'));

        let Some(Popup::Variables(editor)) = &state.popup else {
            panic!("no editor");
        };

        assert_eq!(editor.tabs(), [VarTab::Collection, VarTab::Globals]);

        // nothing changed: Esc closes at once
        press(&mut state, KeyCode::Esc);

        assert!(state.popup.is_none());

        let mut state = with_changed_variables();

        press(&mut state, KeyCode::Esc);

        assert!(asked(&state, false));

        press(&mut state, KeyCode::Esc);

        assert!(matches!(state.popup, Some(Popup::Variables(_))));

        // n leaves without saving
        press(&mut state, KeyCode::Esc);

        assert_eq!(press(&mut state, KeyCode::Char('n')).len(), 0);
        assert!(state.popup.is_none());
        assert!(!state.should_quit);

        // y saves
        let mut state = with_changed_variables();

        press(&mut state, KeyCode::Esc);

        assert_eq!(press(&mut state, KeyCode::Char('y')).len(), 1);
        assert!(state.popup.is_none());
        assert!(!state.should_quit);
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

mod trust {
    use std::path::PathBuf;

    use curst_core::model::Variable;
    use curst_core::vars::VarName;

    use super::*;
    use crate::app::workspace::tests::sample;

    fn id(text: &str) -> LocalId {
        LocalId::new(text).unwrap()
    }

    /// a trusted collection `c1` and an untrusted `.curst/` folder `c2`
    fn with_untrusted() -> State {
        let mut state = State::default();
        let mut shared = sample();

        shared.id = id("c2");
        shared.trusted = false;
        shared.managed = false;
        shared.root = PathBuf::from("/repo/.curst");
        shared.collection.name = "Shared".into();
        shared.collection.variables = vec![Variable::new(VarName::new("host").unwrap(), "evil")];
        update(
            &mut state,
            Action::Loaded {
                collections: vec![sample(), shared],
                globals: vec![],
                problems: vec![],
                orphans: vec![(id("old"), PathBuf::from("/gone"))],
            },
        );

        state
    }

    fn key(collection: &str, path: &[&str]) -> NodeKey {
        NodeKey {
            collection: id(collection),
            path: path.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    #[test]
    fn an_untrusted_folder_asks_at_startup() {
        let mut state = with_untrusted();

        assert!(
            matches!(&state.popup, Some(Popup::Trust { collection }) if *collection == id("c2"))
        );
        assert_eq!(state.sidebar.selected, Some(key("c1", &[])));

        press(&mut state, KeyCode::Char('n'));

        assert!(state.popup.is_none());
        assert!(
            state
                .notice
                .as_deref()
                .unwrap()
                .starts_with("Shared is not trusted")
        );
    }

    #[test]
    fn its_requests_are_not_sent_and_its_variables_not_used() {
        let mut state = with_untrusted();

        press(&mut state, KeyCode::Esc);
        state.focus = Focus::Sidebar;
        state.sidebar.selected = Some(key("c2", &["create"]));
        press(&mut state, KeyCode::Enter);
        state.focus = Focus::Url;
        type_text(&mut state, "http://h/");

        assert_eq!(press(&mut state, KeyCode::Enter), Vec::new());
        assert!(state.notice.as_deref().unwrap().contains("not trusted"));

        // a new request with that collection selected gets none of its
        // variables, and is sent as typed
        state.open = None;
        state.sidebar.selected = Some(key("c2", &[]));
        state.url = editor::single_line("http://{{host}}/", "");

        assert_eq!(press(&mut state, KeyCode::Enter), Vec::new());
        assert_eq!(
            state.notice.as_deref(),
            Some("not sent: {{host}} not defined")
        );

        state.url = editor::single_line("http://plain/", "");

        assert_eq!(
            sent(&press(&mut state, KeyCode::Enter)).1.url,
            "http://plain/"
        );
    }

    #[test]
    fn trusting_or_taking_over_old_secrets() {
        let mut state = with_untrusted();

        assert_eq!(
            press(&mut state, KeyCode::Char('y')),
            [Command::Trust {
                collection: id("c2"),
                reattach: None,
            }]
        );

        // from the sidebar, picking the folder it was moved from
        state.focus = Focus::Sidebar;
        state.sidebar.selected = Some(key("c2", &[]));
        press(&mut state, KeyCode::Char('T'));

        assert_eq!(press(&mut state, KeyCode::Char('5')), Vec::new());
        assert!(state.popup.is_some());
        assert_eq!(
            press(&mut state, KeyCode::Char('1')),
            [Command::Trust {
                collection: id("c2"),
                reattach: Some(id("old")),
            }]
        );

        state.sidebar.expanded.insert(key("c2", &["users"]));
        state.sidebar.selected = Some(key("c2", &["users"]));

        let mut taken = sample();

        taken.id = id("old");
        update(
            &mut state,
            Action::Trusted {
                previous: id("c2"),
                result: Ok(Box::new(taken)),
            },
        );

        assert_eq!(state.sidebar.selected, Some(key("old", &["users"])));
        assert!(state.sidebar.expanded.contains(&key("old", &["users"])));
        assert!(state.workspace(&id("c2")).is_none());
        assert!(state.workspace(&id("old")).is_some_and(|w| w.trusted));
        assert_eq!(state.orphans, Vec::new());

        press(&mut state, KeyCode::Char('T'));

        assert_eq!(
            state.notice.as_deref(),
            Some("this collection is already trusted")
        );

        update(
            &mut state,
            Action::Trusted {
                previous: id("x"),
                result: Err("nope".into()),
            },
        );

        assert_eq!(state.notice.as_deref(), Some("nope"));
    }

    #[test]
    fn a_folder_curst_did_not_make_is_not_offered_for_deletion() {
        let mut state = with_untrusted();

        state.popup = None;
        state.focus = Focus::Sidebar;
        state.sidebar.selected = Some(key("c2", &[]));
        press(&mut state, KeyCode::Char('d'));

        assert!(state.popup.is_none());
        assert!(
            state
                .notice
                .as_deref()
                .unwrap()
                .contains("does not delete /repo/.curst")
        );
    }
}

mod secrets {
    use curst_core::http::StoppedRedirect;
    use curst_core::model::{Body, EnvId, Variable};
    use curst_core::secrets::{Approval, Secret, SecretKey};
    use curst_core::store::Backend;
    use curst_core::vars::VarName;

    use super::*;
    use crate::app::workspace::tests::sample;

    fn token_key() -> SecretKey {
        SecretKey::Environment {
            collection: LocalId::new("c1").unwrap(),
            environment: EnvId::new("e1").unwrap(),
            name: VarName::new("token").unwrap(),
        }
    }

    /// a collection whose active environment holds the secret `token`, and
    /// a URL that uses it
    fn with_secret() -> State {
        let mut state = State::default();
        let mut workspace = sample();

        workspace.active_environment = EnvId::new("e1");
        workspace.collection.environments[0].variables =
            vec![Variable::secret(VarName::new("token").unwrap())];
        state.collection = LocalId::new("c1");
        update(
            &mut state,
            Action::Loaded {
                collections: vec![workspace],
                globals: vec![],
                problems: vec![],
                orphans: vec![],
            },
        );
        type_text(&mut state, "https://api.example.com/?t={{token}}");

        state
    }

    fn load_id(commands: &[Command]) -> u64 {
        match commands {
            [Command::LoadSecrets { id, keys }] => {
                assert_eq!(keys, &[token_key()]);

                *id
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn the_first_secret_asks_for_a_backend_and_a_new_passphrase_twice() {
        let mut state = with_secret();
        let id = load_id(&press(&mut state, KeyCode::Enter));

        assert!(state.in_flight.is_none());

        update(
            &mut state,
            Action::SecretsLocked {
                id: Some(id),
                locked: Locked::Backend,
            },
        );

        assert!(matches!(state.popup, Some(Popup::ChooseBackend)));
        assert_eq!(
            press(&mut state, KeyCode::Char('f')),
            [Command::ChooseBackend(Backend::EncryptedFile)]
        );

        let id = load_id(&update(
            &mut state,
            Action::BackendChosen(Ok(Backend::EncryptedFile)),
        ));

        update(
            &mut state,
            Action::SecretsLocked {
                id: Some(id),
                locked: Locked::Passphrase {
                    new: true,
                    error: None,
                },
            },
        );

        // empty, then two that differ, then two that match
        assert_eq!(press(&mut state, KeyCode::Enter), Vec::new());
        assert!(
            matches!(&state.popup, Some(Popup::Passphrase { error: Some(e), .. }) if e.contains("empty"))
        );

        type_text(&mut state, "one");
        press(&mut state, KeyCode::Enter);
        type_text(&mut state, "two");
        press(&mut state, KeyCode::Enter);

        assert!(
            matches!(&state.popup, Some(Popup::Passphrase { error: Some(e), first: None, .. }) if e.contains("did not match"))
        );

        type_text(&mut state, "pass");
        press(&mut state, KeyCode::Enter);
        type_text(&mut state, "pass");

        assert_eq!(
            press(&mut state, KeyCode::Enter),
            [Command::Unlock(Secret::new("pass"))]
        );

        let id = load_id(&update(&mut state, Action::Unlocked));
        let commands = update(
            &mut state,
            Action::SecretsLoaded {
                id,
                values: vec![(token_key(), Some(Secret::new("s3cr3t")))],
            },
        );
        let [
            Command::Send {
                request, secrets, ..
            },
        ] = commands.as_slice()
        else {
            panic!("not sent: {commands:?}");
        };

        assert_eq!(request.url, "https://api.example.com/?t=s3cr3t");
        assert_eq!(secrets, &[token_key()]);

        // the wait is over, so its message goes
        assert_eq!(state.notice, None);

        // the value is kept for the session: the next send needs no load, and
        // only cancels the one still in flight
        assert!(matches!(
            press(&mut state, KeyCode::Enter).as_slice(),
            [Command::Cancel, Command::Send { .. }]
        ));
    }

    #[test]
    fn a_wrong_passphrase_asks_again_and_esc_gives_up() {
        let mut state = with_secret();
        let id = load_id(&press(&mut state, KeyCode::Enter));

        update(
            &mut state,
            Action::SecretsLocked {
                id: Some(id),
                locked: Locked::Passphrase {
                    new: false,
                    error: None,
                },
            },
        );
        type_text(&mut state, "nope");

        assert_eq!(
            press(&mut state, KeyCode::Enter),
            [Command::Unlock(Secret::new("nope"))]
        );

        update(
            &mut state,
            Action::SecretsLocked {
                id: None,
                locked: Locked::Passphrase {
                    new: false,
                    error: Some("wrong passphrase, try again".into()),
                },
            },
        );

        assert!(matches!(
            &state.popup,
            Some(Popup::Passphrase { error: Some(_), .. })
        ));

        press(&mut state, KeyCode::Esc);

        assert!(state.waiting.is_none());
        assert!(
            state
                .notice
                .as_deref()
                .unwrap()
                .contains("nothing was done")
        );

        // a store that cannot open drops the request with its reason
        let id = load_id(&press(&mut state, KeyCode::Enter));

        update(
            &mut state,
            Action::SecretsLocked {
                id: Some(id),
                locked: Locked::Failed("no keychain".into()),
            },
        );

        assert!(state.popup.is_none());
        assert!(state.waiting.is_none());
        assert_eq!(state.notice.as_deref(), Some("no keychain"));
    }

    #[test]
    fn stale_replies_and_canceled_waits_are_ignored() {
        let mut state = with_secret();
        let id = load_id(&press(&mut state, KeyCode::Enter));

        assert_eq!(
            update(
                &mut state,
                Action::SecretsLoaded {
                    id: id + 10,
                    values: vec![],
                },
            ),
            Vec::new()
        );

        update(
            &mut state,
            Action::SecretsLocked {
                id: Some(id + 10),
                locked: Locked::Backend,
            },
        );

        assert!(state.popup.is_none());

        state.focus = Focus::Response;
        press(&mut state, KeyCode::Esc);

        assert!(state.waiting.is_none());
        assert_eq!(
            update(
                &mut state,
                Action::SecretsLoaded {
                    id,
                    values: vec![(token_key(), Some(Secret::new("v")))],
                },
            ),
            Vec::new()
        );
    }

    #[test]
    fn a_secret_with_no_value_is_named() {
        let mut state = with_secret();
        let id = load_id(&press(&mut state, KeyCode::Enter));

        assert_eq!(
            update(
                &mut state,
                Action::SecretsLoaded {
                    id,
                    values: vec![(token_key(), None)],
                },
            ),
            Vec::new()
        );
        assert!(
            state
                .notice
                .as_deref()
                .unwrap()
                .contains("{{token}} secret with no value yet")
        );
    }

    #[test]
    fn a_new_origin_asks_before_anything_is_sent() {
        let mut state = with_secret();

        state
            .secret_values
            .insert(token_key(), Some(Secret::new("v")));

        let commands = press(&mut state, KeyCode::Enter);
        let (id, _) = sent(&commands);
        let approval = Approval {
            origin: "https://api.example.com:443".into(),
            keys: vec![token_key()],
        };

        update(
            &mut state,
            Action::NeedsApproval {
                id,
                approval: Box::new(approval.clone()),
            },
        );

        assert!(state.in_flight.is_none());
        assert!(matches!(&state.popup, Some(Popup::Approve(a)) if *a == approval));
        assert_eq!(
            press(&mut state, KeyCode::Char('y')),
            [Command::Approve {
                id,
                keys: vec![token_key()],
                origin: approval.origin.clone(),
            }]
        );
        assert!(matches!(
            update(&mut state, Action::Approved { id, result: Ok(()) }).as_slice(),
            [Command::Send { .. }]
        ));

        // saying no sends nothing
        let commands = press(&mut state, KeyCode::Enter);
        let (id, _) = sent(&commands);

        update(
            &mut state,
            Action::NeedsApproval {
                id,
                approval: Box::new(approval),
            },
        );

        assert_eq!(press(&mut state, KeyCode::Char('n')), Vec::new());
        assert!(state.waiting.is_none());
        assert_eq!(
            update(&mut state, Action::Approved { id, result: Ok(()) }),
            Vec::new()
        );
    }

    #[test]
    fn a_stopped_redirect_is_followed_on_request() {
        let mut state = with_secret();

        state
            .secret_values
            .insert(token_key(), Some(Secret::new("v")));
        state.method = Method::Post;
        state.body = BodyEditor::from_body(&Body::Json("{}".into()));

        let commands = press(&mut state, KeyCode::Enter);
        let (id, _) = sent(&commands);
        let mut stopped = response("{}");

        stopped.status = 307;
        stopped.stopped = Some(StoppedRedirect {
            url: "https://other.example.com/next".into(),
            method: Method::Post,
            keeps_body: true,
        });
        update(&mut state, Action::responded(id, Ok(stopped)));

        assert!(state.notice.as_deref().unwrap().contains("press f"));

        state.focus = Focus::Response;

        let commands = press(&mut state, KeyCode::Char('f'));
        let (_, request) = sent(&commands);

        assert_eq!(request.url, "https://other.example.com/next");
        assert_eq!(request.method, Method::Post);
        assert_eq!(request.body, Body::Json("{}".into()));

        let id = state.in_flight.unwrap();

        update(&mut state, Action::responded(id, Ok(response("{}"))));

        assert_eq!(press(&mut state, KeyCode::Char('f')), Vec::new());
        assert_eq!(
            state.notice.as_deref(),
            Some("there is no redirect to follow")
        );
    }
}

mod secret_variables {
    use curst_core::model::{EnvId, KeyValue, Variable};
    use curst_core::secrets::{Secret, SecretKey, SecretScope};
    use curst_core::vars::VarName;

    use super::*;
    use crate::app::workspace::tests::sample;

    fn name(text: &str) -> VarName {
        VarName::new(text).unwrap()
    }

    fn collection_key(text: &str) -> SecretKey {
        SecretKey::Collection {
            collection: LocalId::new("c1").unwrap(),
            name: name(text),
        }
    }

    fn with_variables(variables: Vec<Variable>) -> State {
        let mut state = State::default();
        let mut workspace = sample();

        workspace.collection.variables = variables;
        state.collection = LocalId::new("c1");
        update(
            &mut state,
            Action::Loaded {
                collections: vec![workspace],
                globals: vec![],
                problems: vec![],
                orphans: vec![],
            },
        );

        state
    }

    fn open_editor(state: &mut State) {
        press_key(state, ctrl('e'));
        press(state, KeyCode::Char('v'));
    }

    fn changes(commands: &[Command]) -> (u64, Vec<SecretChange>) {
        match commands {
            [Command::ChangeSecrets { id, changes }] => (*id, changes.clone()),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn marking_a_variable_secret_stores_its_value_before_the_file() {
        let mut state = with_variables(vec![Variable::new(name("key"), "abc")]);

        open_editor(&mut state);
        press(&mut state, KeyCode::Char('s'));

        let (id, changes) = changes(&press_key(&mut state, ctrl('s')));

        assert_eq!(
            changes,
            [SecretChange::Set(collection_key("key"), Secret::new("abc"))]
        );

        // the file is written only once the store took the value
        assert_eq!(
            update(&mut state, Action::SecretsChanged { id, result: Ok(()) },),
            [Command::Store {
                collection: LocalId::new("c1").unwrap(),
                change: Change::SaveVariables(vec![Variable::secret(name("key"))]),
            }]
        );
    }

    #[test]
    fn a_failed_store_writes_no_file() {
        let mut state = with_variables(vec![Variable::new(name("key"), "abc")]);

        open_editor(&mut state);
        press(&mut state, KeyCode::Char('s'));

        let (id, _) = changes(&press_key(&mut state, ctrl('s')));

        assert_eq!(
            update(
                &mut state,
                Action::SecretsChanged {
                    id,
                    result: Err("locked".into()),
                },
            ),
            Vec::new()
        );
        assert_eq!(state.notice.as_deref(), Some("nothing was saved: locked"));
    }

    #[test]
    fn renaming_retyping_and_removing_secrets() {
        let mut state = with_variables(vec![
            Variable::secret(name("old")),
            Variable::secret(name("gone")),
        ]);

        open_editor(&mut state);

        // rename the first and give it a new value, blind
        press(&mut state, KeyCode::Enter);

        for _ in 0.."old".len() {
            press(&mut state, KeyCode::Backspace);
        }

        type_text(&mut state, "new");
        press(&mut state, KeyCode::Tab);
        type_text(&mut state, "v2");
        press(&mut state, KeyCode::Enter);

        // and remove the second
        press(&mut state, KeyCode::Down);
        press(&mut state, KeyCode::Char('d'));

        let (_, changes) = changes(&press_key(&mut state, ctrl('s')));

        assert_eq!(
            changes,
            [
                SecretChange::Rename(collection_key("old"), collection_key("new")),
                SecretChange::Set(collection_key("new"), Secret::new("v2")),
                SecretChange::Delete(collection_key("gone")),
            ]
        );
    }

    #[test]
    fn a_flagged_header_becomes_a_secret() {
        let mut state = with_variables(vec![]);

        state.focus = Focus::Request;
        state.request_tab = RequestTab::Headers;
        state.headers = Table::from_pairs(&[KeyValue::new("Authorization", "Bearer abc123")]);

        press(&mut state, KeyCode::Char('m'));

        let Some(Popup::Prompt { input, .. }) = &state.popup else {
            panic!("no prompt");
        };

        assert_eq!(editor::text_of(input), "authorization");

        press(&mut state, KeyCode::Backspace);
        press(&mut state, KeyCode::Char('!'));

        // a name that is not valid asks again
        assert_eq!(press(&mut state, KeyCode::Enter), Vec::new());
        assert!(state.popup.is_some());

        press(&mut state, KeyCode::Backspace);
        press(&mut state, KeyCode::Char('n'));

        let (id, changes) = changes(&press(&mut state, KeyCode::Enter));

        assert_eq!(
            changes,
            [SecretChange::Set(
                collection_key("authorization"),
                Secret::new("abc123")
            )]
        );
        assert_eq!(state.headers.rows[0].value, "Bearer {{authorization}}");

        let files = update(&mut state, Action::SecretsChanged { id, result: Ok(()) });

        assert_eq!(
            files,
            [Command::Store {
                collection: LocalId::new("c1").unwrap(),
                change: Change::SaveVariables(vec![Variable::secret(name("authorization"))]),
            }]
        );

        // a value that is not flagged is left alone
        state.headers = Table::from_pairs(&[KeyValue::new("Accept", "*/*")]);

        assert_eq!(press(&mut state, KeyCode::Char('m')), Vec::new());
        assert!(state.popup.is_none());
    }

    #[test]
    fn without_a_collection_it_becomes_a_global_secret() {
        let mut state = State {
            focus: Focus::Request,
            request_tab: RequestTab::Auth,
            auth: AuthEditor::from_auth(&curst_core::model::Auth::Bearer {
                token: format!("ghp_{}", "x0".repeat(18)),
            }),
            ..State::default()
        };

        press(&mut state, KeyCode::Char('m'));

        let (_, changes) = changes(&press(&mut state, KeyCode::Enter));

        assert!(matches!(
            changes.as_slice(),
            [SecretChange::Set(SecretKey::Global { .. }, _)]
        ));
        assert_eq!(
            state.auth.auth(),
            curst_core::model::Auth::Bearer {
                token: "{{token}}".into()
            }
        );
    }

    #[test]
    fn an_untrusted_collection_gets_no_secret() {
        let mut state = with_variables(vec![]);

        state.collections[0].trusted = false;
        state.popup = None;
        state.focus = Focus::Request;
        state.request_tab = RequestTab::Headers;
        state.headers = Table::from_pairs(&[KeyValue::new("X-Api-Key", "abc")]);
        press(&mut state, KeyCode::Char('m'));

        assert_eq!(press(&mut state, KeyCode::Enter), Vec::new());
        assert_eq!(state.headers.rows[0].value, "abc");
        assert!(state.notice.as_deref().unwrap().contains("trust"));
    }

    #[test]
    fn deleting_takes_the_secrets_along_and_says_how_many() {
        let mut state = with_variables(vec![Variable::secret(name("key"))]);

        state.collections[0].collection.environments[0].variables =
            vec![Variable::secret(name("token"))];
        state.focus = Focus::Sidebar;
        press(&mut state, KeyCode::Char('d'));

        let Some(Popup::Confirm { message, .. }) = &state.popup else {
            panic!("no question");
        };

        assert!(message.ends_with("and 2 secrets?"), "{message}");

        press(&mut state, KeyCode::Char('y'));

        let commands = update(
            &mut state,
            Action::Stored {
                collection: LocalId::new("c1").unwrap(),
                result: Ok(Box::default()),
            },
        );

        assert_eq!(
            changes(&commands).1,
            [SecretChange::DeleteScope(SecretScope::Collection(
                LocalId::new("c1").unwrap()
            ))]
        );
    }

    #[test]
    fn deleting_an_environment_takes_its_secrets() {
        let mut state = with_variables(vec![]);

        state.collections[0].collection.environments[0].variables =
            vec![Variable::secret(name("token"))];

        let mut after = state.collections[0].clone();

        after.collection.environments.clear();

        let commands = update(
            &mut state,
            Action::Stored {
                collection: LocalId::new("c1").unwrap(),
                result: Ok(Box::new(Stored {
                    workspace: Some(after),
                    ..Stored::default()
                })),
            },
        );

        assert_eq!(
            changes(&commands).1,
            [SecretChange::DeleteScope(SecretScope::Environment(
                LocalId::new("c1").unwrap(),
                EnvId::new("e1").unwrap()
            ))]
        );
    }
}

mod settings_and_editor {
    use curst_core::model::Body;
    use curst_core::secrets::Secret;
    use curst_core::store::Backend;

    use super::*;

    fn ctrl_o(state: &mut State) {
        press_key(state, ctrl('o'));
    }

    #[test]
    fn with_no_backend_settings_offers_the_choice() {
        let mut state = State::default();

        ctrl_o(&mut state);

        assert!(matches!(state.popup, Some(Popup::Settings)));

        press(&mut state, KeyCode::Char('m'));

        assert!(matches!(state.popup, Some(Popup::ChooseBackend)));
        assert_eq!(
            press(&mut state, KeyCode::Char('k')),
            [Command::ChooseBackend(Backend::Keychain)]
        );

        update(&mut state, Action::BackendChosen(Ok(Backend::Keychain)));

        assert_eq!(state.backend, Some(Backend::Keychain));
    }

    #[test]
    fn moving_from_the_keychain_asks_for_a_new_passphrase_twice() {
        let mut state = State::default();

        update(&mut state, Action::BackendKnown(Some(Backend::Keychain)));
        ctrl_o(&mut state);
        press(&mut state, KeyCode::Char('m'));
        type_text(&mut state, "pw");
        press(&mut state, KeyCode::Enter);
        type_text(&mut state, "pw");

        let commands = press(&mut state, KeyCode::Enter);
        let [Command::Migrate { id, to, passphrase }] = commands.as_slice() else {
            panic!("no move: {commands:?}");
        };

        assert_eq!(
            (*to, passphrase.clone()),
            (Backend::EncryptedFile, Some(Secret::new("pw")))
        );

        let id = *id;

        update(
            &mut state,
            Action::Migrated {
                id,
                result: Ok((Backend::EncryptedFile, 3)),
            },
        );

        assert_eq!(state.backend, Some(Backend::EncryptedFile));
        assert_eq!(
            state.notice.as_deref(),
            Some("3 secrets moved to the encrypted file")
        );
    }

    #[test]
    fn moving_to_the_keychain_waits_for_the_file_to_open() {
        let mut state = State::default();

        update(
            &mut state,
            Action::BackendKnown(Some(Backend::EncryptedFile)),
        );
        ctrl_o(&mut state);

        let commands = press(&mut state, KeyCode::Char('m'));
        let [
            Command::Migrate {
                id,
                to: Backend::Keychain,
                passphrase: None,
            },
        ] = commands.as_slice()
        else {
            panic!("no move: {commands:?}");
        };

        update(
            &mut state,
            Action::SecretsLocked {
                id: Some(*id),
                locked: Locked::Passphrase {
                    new: false,
                    error: None,
                },
            },
        );
        type_text(&mut state, "pw");

        assert_eq!(
            press(&mut state, KeyCode::Enter),
            [Command::Unlock(Secret::new("pw"))]
        );

        // once open, the move starts again by itself
        let commands = update(&mut state, Action::Unlocked);

        assert!(matches!(
            commands.as_slice(),
            [Command::Migrate {
                to: Backend::Keychain,
                ..
            }]
        ));

        let [Command::Migrate { id, .. }] = commands.as_slice() else {
            unreachable!()
        };

        update(
            &mut state,
            Action::Migrated {
                id: *id,
                result: Err("the keychain cannot be used here".into()),
            },
        );

        assert_eq!(state.backend, Some(Backend::EncryptedFile));
        assert_eq!(
            state.notice.as_deref(),
            Some("the keychain cannot be used here")
        );
    }

    #[test]
    fn the_body_goes_to_the_editor_as_typed() {
        let mut state = State {
            focus: Focus::Request,
            request_tab: RequestTab::Body,
            ..State::default()
        };

        press(&mut state, KeyCode::Char('E'));

        assert!(state.notice.as_deref().unwrap().contains("JSON or text"));

        state.body = BodyEditor::from_body(&Body::Json("{\"t\": \"{{token}}\"}".into()));

        assert_eq!(
            press(&mut state, KeyCode::Char('E')),
            [Command::EditBody {
                text: "{\"t\": \"{{token}}\"}".into(),
                extension: "json",
            }]
        );

        update(&mut state, Action::BodyEdited(Ok("{\n}\n".into())));

        assert_eq!(state.request().body, Body::Json("{\n}\n".into()));

        update(&mut state, Action::BodyEdited(Err("vi failed".into())));

        assert_eq!(state.request().body, Body::Json("{\n}\n".into()));
        assert_eq!(state.notice.as_deref(), Some("vi failed"));
    }
}

mod plain_again {
    use curst_core::model::Variable;
    use curst_core::secrets::{Secret, SecretKey};
    use curst_core::vars::VarName;

    use super::*;
    use crate::app::workspace::tests::sample;

    fn key() -> SecretKey {
        SecretKey::Collection {
            collection: LocalId::new("c1").unwrap(),
            name: VarName::new("token").unwrap(),
        }
    }

    fn editing_a_secret() -> State {
        let mut state = State::default();
        let mut workspace = sample();

        workspace.collection.variables = vec![Variable::secret(VarName::new("token").unwrap())];
        update(
            &mut state,
            Action::Loaded {
                collections: vec![workspace],
                globals: vec![],
                problems: vec![],
                orphans: vec![],
            },
        );
        press_key(&mut state, ctrl('e'));
        press(&mut state, KeyCode::Char('v'));

        state
    }

    fn row(state: &State) -> (bool, String) {
        let Some(Popup::Variables(editor)) = &state.popup else {
            panic!("no editor");
        };

        let row = &editor.table().rows[0];

        (row.secret, row.value.clone())
    }

    #[test]
    fn a_secret_seen_this_session_turns_plain_at_once() {
        let mut state = editing_a_secret();

        state.secret_values.insert(key(), Some(Secret::new("t0k")));

        assert_eq!(press(&mut state, KeyCode::Char('s')), Vec::new());
        assert_eq!(row(&state), (false, "t0k".into()));

        // saving deletes the secret first, then writes the plain value
        let commands = press_key(&mut state, ctrl('s'));
        let [Command::ChangeSecrets { id, changes }] = commands.as_slice() else {
            panic!("unexpected {commands:?}");
        };

        assert_eq!(changes, &[SecretChange::Delete(key())]);
        assert_eq!(
            update(
                &mut state,
                Action::SecretsChanged {
                    id: *id,
                    result: Ok(()),
                },
            ),
            [Command::Store {
                collection: LocalId::new("c1").unwrap(),
                change: Change::SaveVariables(vec![Variable::new(
                    VarName::new("token").unwrap(),
                    "t0k"
                )]),
            }]
        );
    }

    #[test]
    fn otherwise_the_value_is_fetched_and_the_editor_comes_back() {
        let mut state = editing_a_secret();
        let commands = press(&mut state, KeyCode::Char('s'));
        let [Command::LoadSecrets { id, keys }] = commands.as_slice() else {
            panic!("unexpected {commands:?}");
        };

        assert_eq!(keys, &[key()]);
        assert!(state.popup.is_none());

        update(
            &mut state,
            Action::SecretsLoaded {
                id: *id,
                values: vec![(key(), Some(Secret::new("t0k")))],
            },
        );

        assert_eq!(row(&state), (false, "t0k".into()));
        assert!(global_hints_listed(&state));
    }

    #[test]
    fn canceling_the_passphrase_gives_the_editor_back_unchanged() {
        let mut state = editing_a_secret();
        let commands = press(&mut state, KeyCode::Char('s'));
        let [Command::LoadSecrets { id, .. }] = commands.as_slice() else {
            panic!("unexpected {commands:?}");
        };

        update(
            &mut state,
            Action::SecretsLocked {
                id: Some(*id),
                locked: Locked::Passphrase {
                    new: false,
                    error: None,
                },
            },
        );
        press(&mut state, KeyCode::Esc);

        assert_eq!(row(&state), (true, String::new()));

        // a store that cannot open gives it back too
        let commands = press(&mut state, KeyCode::Char('s'));
        let [Command::LoadSecrets { id, .. }] = commands.as_slice() else {
            panic!("unexpected {commands:?}");
        };

        update(
            &mut state,
            Action::SecretsLocked {
                id: Some(*id),
                locked: Locked::Failed("no keychain".into()),
            },
        );

        assert_eq!(row(&state), (true, String::new()));
        assert_eq!(state.notice.as_deref(), Some("no keychain"));
    }

    #[test]
    fn undoing_a_secret_made_in_this_editor_keeps_the_typed_value() {
        let mut state = editing_a_secret();

        press(&mut state, KeyCode::Char('a'));
        type_text(&mut state, "plain");
        press(&mut state, KeyCode::Tab);
        type_text(&mut state, "v");
        press(&mut state, KeyCode::Enter);
        press(&mut state, KeyCode::Char('s'));
        press(&mut state, KeyCode::Char('s'));

        let Some(Popup::Variables(editor)) = &state.popup else {
            panic!("no editor");
        };

        let row = &editor.table().rows[1];

        assert_eq!((row.secret, row.value.as_str()), (false, "v"));
    }

    fn global_hints_listed(state: &State) -> bool {
        let has = |hints: Vec<crate::app::Hint>, key: &str, label: &str| {
            hints.iter().any(|h| h.key == key && h.label == label)
        };

        has(crate::ui::hints(state), "s", "secret")
            && has(crate::ui::global_hints(state), "Esc", "close")
    }
}

mod tabs {
    use curst_core::model::{EnvId, Variable};
    use curst_core::vars::VarName;

    use super::*;
    use crate::app::workspace::tests::sample;

    fn node(path: &[&str]) -> NodeKey {
        NodeKey {
            collection: LocalId::new("c1").unwrap(),
            path: path.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    fn loaded() -> State {
        let mut state = State::default();

        update(
            &mut state,
            Action::Loaded {
                collections: vec![sample()],
                globals: vec![],
                problems: vec![],
                orphans: vec![],
            },
        );

        state
    }

    fn open(state: &mut State, path: &[&str]) {
        state.focus = Focus::Sidebar;
        state.sidebar.expanded.insert(node(&["users"]));
        state.sidebar.selected = Some(node(path));
        press(state, KeyCode::Enter);
    }

    fn names(state: &State) -> Vec<String> {
        state
            .tab_labels()
            .into_iter()
            .map(|l| {
                let mark = if l.active { "*" } else { "" };

                format!("{mark}{}", l.name)
            })
            .collect()
    }

    fn ctrl_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn requests_open_in_tabs_and_an_open_one_is_reused() {
        let mut state = loaded();

        // the blank first tab is used for the first request
        open(&mut state, &["create"]);

        assert_eq!(names(&state), ["*CREATE"]);

        open(&mut state, &["users", "list"]);

        assert_eq!(names(&state), ["CREATE", "*LIST"]);

        open(&mut state, &["create"]);

        assert_eq!(names(&state), ["*CREATE", "LIST"]);
        assert_eq!(state.method, Method::Post);
    }

    #[test]
    fn switching_with_ctrl_arrows_and_numbers() {
        let mut state = loaded();

        press_key(&mut state, ctrl('t'));
        press_key(&mut state, ctrl('t'));
        type_text(&mut state, "http://third/");

        assert_eq!(names(&state), ["new request", "new request", "*third/"]);

        press_key(&mut state, ctrl_key(KeyCode::Right));

        assert_eq!(state.active_tab, 0);

        press_key(&mut state, ctrl_key(KeyCode::Left));

        assert_eq!(state.active_tab, 2);
        assert_eq!(state.request().url, "http://third/");

        press_key(&mut state, ctrl('1'));

        assert_eq!((state.active_tab, state.request().url), (0, String::new()));

        press_key(&mut state, ctrl('9'));

        assert_eq!(state.active_tab, 2);

        press_key(&mut state, ctrl('2'));

        assert_eq!(state.active_tab, 1);

        // a number past the last tab goes to the last
        press_key(&mut state, ctrl('8'));

        assert_eq!(state.active_tab, 2);
    }

    #[test]
    fn ctrl_g_lists_the_tabs_and_switches_in_every_terminal() {
        let mut state = loaded();

        for url in ["http://one/", "http://two/", "http://three/"] {
            if url != "http://one/" {
                press_key(&mut state, ctrl('t'));
            }

            type_text(&mut state, url);
        }

        press_key(&mut state, ctrl('g'));

        assert!(matches!(state.popup, Some(Popup::Tabs { selected: 2 })));

        // a number jumps, and the list closes
        press(&mut state, KeyCode::Char('1'));

        assert!(state.popup.is_none());
        assert_eq!(state.request().url, "http://one/");

        // 9 is always the last
        press_key(&mut state, ctrl('g'));
        press(&mut state, KeyCode::Char('9'));

        assert_eq!(state.request().url, "http://three/");

        // arrows wrap, Enter opens
        press_key(&mut state, ctrl('g'));
        press(&mut state, KeyCode::Down);
        press(&mut state, KeyCode::Enter);

        assert_eq!(state.request().url, "http://one/");

        // w closes the highlighted tab and keeps the list open
        press_key(&mut state, ctrl('g'));
        press(&mut state, KeyCode::Up);
        press(&mut state, KeyCode::Char('w'));

        assert_eq!(names(&state), ["one/", "*two/"]);
        assert!(matches!(state.popup, Some(Popup::Tabs { .. })));

        // n opens a new tab; Esc or Ctrl+G only close the list
        press(&mut state, KeyCode::Char('n'));

        assert_eq!(state.tab_count(), 3);
        assert!(state.popup.is_none());

        press_key(&mut state, ctrl('g'));
        press_key(&mut state, ctrl('g'));

        assert!(state.popup.is_none());

        press_key(&mut state, ctrl('g'));
        press(&mut state, KeyCode::Esc);

        assert!(state.popup.is_none());
        assert_eq!(state.tab_count(), 3);
    }

    #[test]
    fn closing_a_changed_tab_from_the_list_asks_first() {
        let mut state = loaded();

        open(&mut state, &["create"]);
        state.focus = Focus::Url;
        type_text(&mut state, "x");
        press_key(&mut state, ctrl('t'));
        press_key(&mut state, ctrl('g'));
        press(&mut state, KeyCode::Up);
        press(&mut state, KeyCode::Char('w'));

        assert!(matches!(state.popup, Some(Popup::Confirm { .. })));
        assert_eq!(state.tab_count(), 2);
    }

    #[test]
    fn closing_tabs_keeps_one() {
        let mut state = loaded();

        type_text(&mut state, "http://one/");
        press_key(&mut state, ctrl('t'));
        press_key(&mut state, ctrl('w'));

        assert_eq!(names(&state), ["*one/"]);

        press_key(&mut state, ctrl('w'));

        assert_eq!(names(&state), ["*new request"]);
        assert_eq!(state.request().url, "");

        // closing a tab whose request is running stops it
        type_text(&mut state, "http://slow/");
        press(&mut state, KeyCode::Enter);

        assert_eq!(press_key(&mut state, ctrl('w')), [Command::Cancel]);
    }

    #[test]
    fn a_response_lands_in_the_tab_that_sent_it() {
        let mut state = loaded();

        type_text(&mut state, "http://first/");

        let commands = press(&mut state, KeyCode::Enter);
        let (id, _) = sent(&commands);

        press_key(&mut state, ctrl('t'));
        update(&mut state, Action::responded(id, Ok(response("{}"))));

        assert!(state.outcome.is_none());

        press_key(&mut state, ctrl('1'));

        assert!(matches!(state.outcome, Some(Outcome::Response(_))));
        assert!(state.in_flight.is_none());
    }

    #[test]
    fn leaving_a_collection_keeps_the_request_but_not_its_variables() {
        let mut state = loaded();

        state.collections[0].collection.variables =
            vec![Variable::new(VarName::new("host").unwrap(), "h")];
        state.collections[0].active_environment = EnvId::new("e1");
        open(&mut state, &["create"]);
        state.focus = Focus::Url;
        type_text(&mut state, "http://{{host}}/");

        assert!(
            sent(&press(&mut state, KeyCode::Enter))
                .1
                .url
                .contains("h/")
        );

        press_key(&mut state, ctrl('l'));

        assert!(state.open.is_none());
        assert!(state.collection.is_none());
        assert_eq!(state.method, Method::Post);
        assert_eq!(
            state.notice.as_deref(),
            Some("left the collection: only global variables apply now")
        );
        assert_eq!(press(&mut state, KeyCode::Enter), Vec::new());

        // and it says how to bring them back
        assert!(
            state
                .notice
                .as_deref()
                .unwrap()
                .starts_with("not sent: {{host}} not defined; {{host}} is in Pets, and this request is outside any collection")
        );

        press_key(&mut state, ctrl('l'));

        assert_eq!(
            state.notice.as_deref(),
            Some("this request is not in a collection")
        );
    }

    #[test]
    fn renames_and_deletes_reach_tabs_in_the_background() {
        let mut state = loaded();

        open(&mut state, &["create"]);
        press_key(&mut state, ctrl('t'));

        let mut moved = sample();

        moved.collection.items.retain(|i| i.slug() != "create");
        update(
            &mut state,
            Action::Stored {
                collection: LocalId::new("c1").unwrap(),
                result: Ok(Box::new(Stored {
                    workspace: Some(moved),
                    ..Stored::default()
                })),
            },
        );
        press_key(&mut state, ctrl('1'));

        // the request was deleted: the tab keeps it, unsaved
        assert!(state.open.is_none());
        assert_eq!(state.method, Method::Post);

        // a deleted collection takes its tabs out of it
        open(&mut state, &["users", "list"]);
        press_key(&mut state, ctrl('t'));
        update(
            &mut state,
            Action::Stored {
                collection: LocalId::new("c1").unwrap(),
                result: Ok(Box::default()),
            },
        );
        press_key(&mut state, ctrl('1'));

        assert!(state.collection.is_none());
    }
}

mod curl {
    use curst_core::secrets::SecretKey;
    use curst_core::vars::VarName;

    use super::*;

    const COMMAND: &str = "curl 'https://api.example.com/v1/items?api_key=k123' \\\n  -u ann:pw \\\n  -H 'Accept: application/json'";

    #[test]
    fn a_pasted_command_becomes_a_request_and_its_credentials_secrets() {
        let mut state = State::default();
        let commands = update(&mut state, Action::PasteText(COMMAND.into()));

        assert_eq!(state.request().url, "https://api.example.com/v1/items");
        assert_eq!(state.params.pairs()[0].value, "{{api_key}}");
        assert_eq!(state.headers.pairs()[0].name, "Accept");
        assert_eq!(state.focus, Focus::Url);
        assert!(
            state
                .notice
                .as_deref()
                .unwrap()
                .contains("{{api_key}}, {{password}}")
        );

        // the values go to the secret store, never into the request
        let [Command::ChangeSecrets { changes, .. }] = commands.as_slice() else {
            panic!("{commands:?}");
        };
        let stored: Vec<(String, &str)> = changes
            .iter()
            .map(|change| match change {
                SecretChange::Set(SecretKey::Global { name }, value) => {
                    (name.to_string(), value.expose())
                }
                other => panic!("{other:?}"),
            })
            .collect();

        assert_eq!(
            stored,
            [
                ("api_key".to_owned(), "k123"),
                ("password".to_owned(), "pw")
            ]
        );
        assert!(!format!("{:?}", state.request()).contains("k123"));
    }

    #[test]
    fn a_command_opens_in_a_tab_of_its_own_when_the_current_one_is_used() {
        let mut state = State::default();

        type_text(&mut state, "https://kept.example");
        update(&mut state, Action::PasteText("curl https://h/x".into()));

        assert_eq!(state.tab_count(), 2);
        assert_eq!(state.request().url, "https://h/x");
    }

    #[test]
    fn other_text_goes_into_the_field_on_one_line() {
        let mut state = State::default();

        update(&mut state, Action::PasteText("https://h/a\nb\r\n".into()));

        assert_eq!(state.request().url, "https://h/a b");

        // a command that cannot be read says why, and changes nothing
        update(&mut state, Action::PasteText("curl 'https://h".into()));

        assert_eq!(state.request().url, "https://h/a b");
        assert!(state.notice.as_deref().unwrap().starts_with("not imported"));
    }

    #[test]
    fn a_paste_into_a_popup_stays_in_it() {
        let mut state = State::default();

        update(&mut state, Action::Palette);
        update(&mut state, Action::PasteText(COMMAND.into()));

        let Some(Popup::Palette(palette)) = &state.popup else {
            panic!("the palette closed");
        };

        assert!(crate::app::editor::text_of(&palette.input).starts_with("curl "));
        assert_eq!(state.request().url, "");
    }

    #[test]
    fn copy_as_curl_keeps_variables_and_never_values() {
        let mut state = State::default();

        type_text(&mut state, "https://h/{{version}}");
        state.auth = crate::app::AuthEditor::from_auth(&curst_core::model::Auth::Bearer {
            token: "{{token}}".into(),
        });
        state.secret_values.insert(
            SecretKey::Global {
                name: VarName::new("token").unwrap(),
            },
            Some(curst_core::secrets::Secret::new("t0k-value")),
        );

        let commands = update(&mut state, Action::CopyCurl);
        let [Command::Copy(text)] = commands.as_slice() else {
            panic!("{commands:?}");
        };

        assert!(text.contains("Authorization: Bearer {{token}}"), "{text}");
        assert!(!text.contains("t0k-value"));
        assert!(curst_core::curl::parse(text).is_ok());
    }

    #[test]
    fn copying_the_response_leaves_borders_and_notes_out() {
        let mut state = State::default();

        type_text(&mut state, "http://localhost/");

        let commands = update(&mut state, Action::Send);
        let (id, _) = super::sent(&commands);

        update(
            &mut state,
            Action::responded(id, Ok(response(r#"{"a":1}"#))),
        );

        // y copies the body from any pane, whichever response tab shows
        state.focus = Focus::Sidebar;
        state.response_tab = ResponseTab::Headers;

        let commands = press(&mut state, KeyCode::Char('y'));

        assert!(matches!(
            commands.as_slice(),
            [Command::Copy(text)] if text == "{\n  \"a\": 1\n}"
        ));
        assert!(
            crate::ui::hints(&state)
                .iter()
                .any(|h| h.key == "y" && h.label == "copy body")
        );

        // in the URL, y is a letter
        state.focus = Focus::Url;

        assert_eq!(press(&mut state, KeyCode::Char('y')).len(), 0);
        assert!(state.request().url.ends_with('y'));

        state.outcome = None;

        assert_eq!(update(&mut state, Action::CopyResponse).len(), 0);
        assert_eq!(
            state.notice.as_deref(),
            Some("there is no response to copy")
        );
    }
}

mod bundles {
    use std::path::Path;

    use curst_core::model::Variable;
    use curst_core::secrets::SecretKey;
    use curst_core::vars::VarName;

    use super::*;
    use crate::app::workspace::tests::sample;

    fn name(text: &str) -> VarName {
        VarName::new(text).unwrap()
    }

    /// the sample collection, with a secret in its environment and one of
    /// its own
    fn with_secrets() -> Workspace {
        let mut workspace = sample();

        workspace.collection.variables = vec![Variable::secret(name("token"))];
        workspace.collection.environments[0]
            .variables
            .push(Variable::secret(name("password")));

        workspace
    }

    fn stored(commands: &[Command]) -> Vec<(String, String)> {
        commands
            .iter()
            .flat_map(|c| match c {
                Command::ChangeSecrets { changes, .. } => changes.clone(),
                _ => Vec::new(),
            })
            .map(|change| match change {
                SecretChange::Set(key, value) => {
                    (key.name().unwrap().to_string(), value.expose().to_owned())
                }
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn an_imported_collection_asks_for_its_secrets() {
        let mut state = State::default();

        update(
            &mut state,
            Action::Imported(Ok((Box::new(with_secrets()), Vec::new()))),
        );

        assert_eq!(state.collections.len(), 1);
        assert_eq!(state.notice.as_deref(), Some("imported Pets"));

        let Some(Popup::Fill(fill)) = &state.popup else {
            panic!("no pop-up");
        };
        let labels: Vec<&str> = fill.secrets.iter().map(|(_, l)| l.as_str()).collect();

        assert_eq!(labels, ["password · Dev", "token · the collection"]);

        // the first is typed and kept, the second skipped
        type_text(&mut state, "pw");

        let commands = press(&mut state, KeyCode::Enter);

        assert_eq!(commands.len(), 0);

        let commands = press(&mut state, KeyCode::Enter);

        assert!(state.popup.is_none());
        assert_eq!(
            stored(&commands),
            [("password".to_owned(), "pw".to_owned())]
        );
    }

    #[test]
    fn an_imported_collection_uses_its_first_environment() {
        let mut state = State::default();
        let commands = update(
            &mut state,
            Action::Imported(Ok((Box::new(with_secrets()), Vec::new()))),
        );
        let first = state.collections[0].collection.environments[0].id.clone();

        assert_eq!(state.collections[0].active_environment, Some(first.clone()));
        assert!(matches!(
            commands.as_slice(),
            [Command::SetActiveEnvironment { environment: Some(id), .. }] if *id == first
        ));
    }

    #[test]
    fn nothing_typed_stores_nothing_and_a_paste_stays_in_the_field() {
        let mut state = State::default();

        update(
            &mut state,
            Action::Imported(Ok((Box::new(with_secrets()), Vec::new()))),
        );
        update(&mut state, Action::PasteText("curl https://h".into()));

        let Some(Popup::Fill(fill)) = &state.popup else {
            panic!("the paste left the pop-up");
        };

        assert_eq!(crate::app::editor::text_of(&fill.input), "curl https://h");
        assert_eq!(state.tab_count(), 1);

        update(&mut state, Action::Down);
        update(&mut state, Action::Up);

        let commands = press(&mut state, KeyCode::Esc);

        // what was typed before moving is kept
        assert_eq!(
            stored(&commands),
            [("password".to_owned(), "curl https://h".to_owned())]
        );

        update(
            &mut state,
            Action::Imported(Ok((Box::new(sample()), Vec::new()))),
        );

        assert!(
            state.popup.is_none(),
            "a collection without secrets asks nothing"
        );
    }

    #[test]
    fn a_request_waiting_for_an_empty_secret_goes_once_it_is_filled() {
        let mut state = State::default();

        update(
            &mut state,
            Action::Loaded {
                collections: vec![with_secrets()],
                globals: vec![],
                problems: vec![],
                orphans: vec![],
            },
        );

        let key = SecretKey::Collection {
            collection: state.collections[0].id.clone(),
            name: name("token"),
        };

        state.collection = Some(state.collections[0].id.clone());
        state.collections[0].active_environment = None;
        state.secret_values.insert(key.clone(), None);
        state.focus = Focus::Url;
        type_text(&mut state, "https://h/{{token}}");

        assert_eq!(update(&mut state, Action::Send).len(), 0);
        assert!(matches!(state.popup, Some(Popup::Fill(_))));

        type_text(&mut state, "t0k");

        let commands = press(&mut state, KeyCode::Enter);
        let Some(Command::ChangeSecrets { id, .. }) = commands.first() else {
            panic!("{commands:?}");
        };

        assert_eq!(stored(&commands), [("token".to_owned(), "t0k".to_owned())]);

        // once stored, the request is sent: its secrets are fetched again
        let commands = update(
            &mut state,
            Action::SecretsChanged {
                id: *id,
                result: Ok(()),
            },
        );

        assert!(
            matches!(commands.as_slice(), [Command::LoadSecrets { keys, .. }] if keys == &[key]),
            "{commands:?}"
        );
    }

    #[test]
    fn export_asks_where_and_warns_about_plain_credentials() {
        let mut state = State::default();

        update(
            &mut state,
            Action::Loaded {
                collections: vec![sample()],
                globals: vec![],
                problems: vec![],
                orphans: vec![],
            },
        );
        state.focus = Focus::Sidebar;
        press(&mut state, KeyCode::Char('e'));

        let Some(Popup::Prompt { input, .. }) = &state.popup else {
            panic!("no prompt");
        };

        assert_eq!(crate::app::editor::text_of(input), "pets.curst.toml");

        let commands = press(&mut state, KeyCode::Enter);

        assert!(matches!(
            commands.as_slice(),
            [Command::ExportBundle { path, .. }] if path.as_path() == Path::new("pets.curst.toml")
        ));

        // a plain-text credential is pointed out first
        state.collections[0]
            .collection
            .variables
            .push(Variable::new(name("api_key"), "abc123"));
        press(&mut state, KeyCode::Char('e'));

        assert_eq!(press(&mut state, KeyCode::Enter).len(), 0);
        assert!(matches!(state.popup, Some(Popup::Confirm { .. })));
        assert!(matches!(
            press(&mut state, KeyCode::Char('y')).as_slice(),
            [Command::ExportBundle { .. }]
        ));

        // and import asks for the file
        press(&mut state, KeyCode::Char('i'));
        type_text(&mut state, "shared.curst.toml");

        assert!(matches!(
            press(&mut state, KeyCode::Enter).as_slice(),
            [Command::ImportBundle(path)] if path.as_path() == Path::new("shared.curst.toml")
        ));

        update(&mut state, Action::Imported(Err("no such file".into())));

        assert_eq!(state.notice.as_deref(), Some("not imported: no such file"));
    }
}

mod undefined {
    use curst_core::model::{EnvId, Environment, Variable};
    use curst_core::vars::VarName;

    use super::*;
    use crate::app::workspace::tests::sample;

    /// the sample collection, with a second environment holding `api_key`
    fn with_staging() -> State {
        let mut state = State::default();
        let mut workspace = sample();

        workspace.collection.environments.push(Environment {
            id: EnvId::new("e2").unwrap(),
            slug: "staging".into(),
            name: "Staging".into(),
            variables: vec![Variable::secret(VarName::new("api_key").unwrap())],
        });
        workspace.active_environment = EnvId::new("e1");
        update(
            &mut state,
            Action::Loaded {
                collections: vec![workspace],
                globals: vec![],
                problems: vec![],
                orphans: vec![],
            },
        );

        state
    }

    #[test]
    fn a_variable_in_an_environment_not_in_use_says_so() {
        let mut state = with_staging();

        state.collection = Some(state.collections[0].id.clone());
        type_text(&mut state, "https://h/{{api_key}}");

        assert_eq!(update(&mut state, Action::Send).len(), 0);
        assert_eq!(
            state.notice.as_deref(),
            Some(
                "not sent: {{api_key}} not defined in Dev; {{api_key}} is in the environment Staging, which is not in use: Ctrl+E chooses it"
            )
        );
    }

    #[test]
    fn a_request_outside_the_collection_says_so() {
        let mut state = with_staging();

        type_text(&mut state, "https://h/{{api_key}}");

        assert_eq!(update(&mut state, Action::Send).len(), 0);

        let notice = state.notice.as_deref().unwrap();

        assert!(
            notice.contains("is in Pets, and this request is outside any collection"),
            "{notice}"
        );
        assert!(notice.contains("save it there with Ctrl+S"), "{notice}");

        // a variable no collection has keeps the short notice
        let mut state = with_staging();

        type_text(&mut state, "https://h/{{nowhere}}");
        update(&mut state, Action::Send);

        assert_eq!(
            state.notice.as_deref(),
            Some("not sent: {{nowhere}} not defined")
        );
    }
}
