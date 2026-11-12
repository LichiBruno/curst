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

    for expected in [Focus::Request, Focus::Response, Focus::Method, Focus::Url] {
        press(&mut state, KeyCode::Tab);

        assert_eq!(state.focus, expected);
    }
    press(&mut state, KeyCode::BackTab);

    assert_eq!(state.focus, Focus::Method);
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
    let prompt = state.save_prompt.as_ref().unwrap();

    assert_eq!(editor::text_of(prompt), "response.json");

    press(&mut state, KeyCode::Char('q'));

    assert!(!state.should_quit);
    assert_eq!(
        press(&mut state, KeyCode::Enter),
        vec![Command::SaveBody("response.jsonq".into())]
    );

    update(&mut state, Action::Saved(Err("exists".into())));

    assert!(state.save_prompt.is_some());
    assert_eq!(state.notice.as_deref(), Some("exists"));

    update(&mut state, Action::Saved(Ok("out.json".into())));

    assert!(state.save_prompt.is_none());
    assert_eq!(state.notice.as_deref(), Some("saved to out.json"));

    press(&mut state, KeyCode::Char('s'));
    press(&mut state, KeyCode::Esc);

    assert!(state.save_prompt.is_none());
}
