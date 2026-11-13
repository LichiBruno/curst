use ratatui::crossterm::event::{KeyCode, KeyModifiers};

use super::presets::{STANDARD, VIM};
use super::*;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn every_preset_key_parses_and_names_an_action() {
    for (layer, name, keys) in STANDARD {
        assert!(action(name).is_some(), "{}: {name}", layer.name());

        for key in *keys {
            assert!(parse_sequence(key).is_ok(), "{key}");
        }
    }

    for (_, name, keys) in VIM {
        assert!(action(name).is_some(), "{name}");
        assert!(keys.iter().all(|k| parse_sequence(k).is_ok()), "{name}");
    }

    assert_eq!(Keymap::standard().conflicts(), Vec::<String>::new());
    assert_eq!(Keymap::vim().conflicts(), Vec::<String>::new());
}

#[test]
fn vim_changes_keys_and_keeps_the_rest() {
    let vim = Keymap::vim();
    let panes = [Layer::Response, Layer::Panes, Layer::Global];
    let g = Chord::char('g');

    assert_eq!(vim.lookup(&panes, &[g]), Lookup::Prefix);
    assert_eq!(vim.lookup(&panes, &[g, g]), Lookup::Exact("top".into()));
    assert_eq!(
        vim.lookup(&panes, &[g, Chord::char('t')]),
        Lookup::Exact("request.next".into())
    );
    assert_eq!(
        vim.lookup(&[Layer::Request], &[Chord::char('d'), Chord::char('d')]),
        Lookup::Exact("row.delete".into())
    );
    assert_eq!(
        vim.lookup(&[Layer::Request], &[Chord::char('d')]),
        Lookup::Prefix
    );
    assert_eq!(
        vim.lookup(&[Layer::Global], &[Chord::ctrl('r')]),
        Lookup::Exact("send".into())
    );

    // typing in the URL is untouched: no sequence starts there
    assert_eq!(
        vim.lookup(&[Layer::Url, Layer::Global], &[g]),
        Lookup::Nothing
    );
}

#[test]
fn settings_choose_a_preset_and_override_keys() {
    let bindings = vec![
        ("global.send".to_owned(), vec!["ctrl+enter".to_owned()]),
        ("sidebar.delete".to_owned(), vec![]),
        (
            "request.row.add".to_owned(),
            vec!["+".to_owned(), "nope key".to_owned()],
        ),
        ("send".to_owned(), vec!["x".to_owned()]),
        ("global.fly".to_owned(), vec!["x".to_owned()]),
        ("sidebar.rename".to_owned(), vec!["n".to_owned()]),
    ];
    let (keymap, warnings) = Keymap::from_settings(Some("vim"), &bindings);

    assert_eq!(
        keymap.key_for(Layer::Global, "send").as_deref(),
        Some("Ctrl+Enter")
    );
    assert_eq!(keymap.key_for(Layer::Sidebar, "delete"), None);
    assert_eq!(
        keymap.key_for(Layer::Request, "row.add").as_deref(),
        Some("+")
    );
    assert_eq!(
        keymap.key_for(Layer::Response, "top").as_deref(),
        Some("g g")
    );

    // a bad key, two bad names, and n bound to both rename and new-request
    assert_eq!(warnings.len(), 4, "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("n is bound to both")));

    let (standard, warnings) = Keymap::from_settings(Some("emacs"), &[]);

    assert_eq!(standard, Keymap::standard());
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        Keymap::from_settings(None, &[]),
        (Keymap::standard(), Vec::new())
    );
}

#[test]
fn layers_have_names_both_ways() {
    for layer in Layer::ALL {
        assert_eq!(Layer::named(layer.name()), Some(layer));
    }

    assert_eq!(Layer::named("nope"), None);
}

#[test]
fn numbered_names() {
    assert!(matches!(action("request.go-1"), Some(Action::RequestAt(0))));
    assert!(matches!(action("request.go-8"), Some(Action::RequestAt(7))));
    assert!(matches!(
        action("request.go-last"),
        Some(Action::RequestAt(usize::MAX))
    ));
    assert!(action("request.go-9").is_none());
    assert!(matches!(action("trust.moved-9"), Some(Action::Reattach(8))));
    assert!(action("trust.moved-0").is_none());
    assert!(action("nothing").is_none());
}

#[test]
fn lookups_prefer_complete_bindings_and_earlier_layers() {
    let keymap = Keymap::from_table(&[
        (Layer::Response, "top", &["g g"]),
        (Layer::Global, "request.next", &["g t"]),
        (Layer::Panes, "quit", &["q"]),
        (Layer::Response, "follow", &["q"]),
    ]);
    let layers = [Layer::Response, Layer::Panes, Layer::Global];
    let g = Chord::char('g');

    assert_eq!(keymap.lookup(&layers, &[g]), Lookup::Prefix);
    assert_eq!(
        keymap.lookup(&layers, &[g, Chord::char('t')]),
        Lookup::Exact("request.next".into())
    );
    assert_eq!(
        keymap.lookup(&layers, &[Chord::char('q')]),
        Lookup::Exact("follow".into())
    );
    assert_eq!(keymap.lookup(&layers, &[Chord::char('z')]), Lookup::Nothing);
    assert_eq!(
        keymap.key_for(Layer::Response, "top").as_deref(),
        Some("g g")
    );
    assert_eq!(keymap.key_for(Layer::Url, "top"), None);
}

#[test]
fn conflicts_are_found() {
    let keymap = Keymap::from_table(&[
        (Layer::Sidebar, "rename", &["r"]),
        (Layer::Sidebar, "delete", &["r", "d"]),
        (Layer::Sidebar, "cut", &["d d"]),
        (Layer::Request, "row.add", &["r"]),
    ]);
    let conflicts = keymap.conflicts();

    assert_eq!(conflicts.len(), 2, "{conflicts:?}");
    assert!(conflicts[0].contains("r is bound to both rename and delete"));
}

/// a state whose keymap binds `g t` to the next request tab everywhere
fn with_sequence() -> State {
    State {
        keymap: Keymap::from_table(&[
            (Layer::Global, "request.next", &["g t"]),
            (Layer::Panes, "quit", &["q"]),
        ]),
        ..State::default()
    }
}

#[test]
fn a_sequence_waits_for_its_next_key() {
    let mut state = with_sequence();

    state.focus = Focus::Sidebar;

    let first = action_for(&state, key(KeyCode::Char('g')));

    assert!(matches!(first.as_slice(), [Action::Pending(_)]));

    for action in first {
        super::super::update(&mut state, action);
    }

    assert_eq!(state.pending_keys.len(), 1);
    assert!(matches!(
        action_for(&state, key(KeyCode::Char('t'))).as_slice(),
        [Action::NextRequest]
    ));

    // a key that goes nowhere drops the sequence, and counts on its own
    assert!(matches!(
        action_for(&state, key(KeyCode::Char('q'))).as_slice(),
        [Action::ClearPending, Action::Quit]
    ));
    assert!(matches!(
        action_for(&state, key(KeyCode::Char('z'))).as_slice(),
        [Action::ClearPending]
    ));
}

#[test]
fn in_a_text_field_a_failed_sequence_is_typed() {
    let mut state = with_sequence();

    super::super::update(&mut state, Action::Pending(key(KeyCode::Char('g'))));

    let actions = action_for(&state, key(KeyCode::Char('o')));

    assert!(matches!(
        actions.as_slice(),
        [
            Action::ClearPending,
            Action::Type(KeyEvent {
                code: KeyCode::Char('g'),
                ..
            }),
            Action::Type(KeyEvent {
                code: KeyCode::Char('o'),
                ..
            })
        ]
    ));

    for action in actions {
        super::super::update(&mut state, action);
    }

    assert_eq!(state.request().url, "go");
    assert_eq!(state.pending_keys.len(), 0);
}

#[test]
fn unbound_keys_are_typed_only_in_text_fields() {
    let mut state = State::default();

    assert!(matches!(
        action_for(&state, key(KeyCode::Char('x'))).as_slice(),
        [Action::Type(_)]
    ));

    state.focus = Focus::Sidebar;

    assert_eq!(action_for(&state, key(KeyCode::Char('z'))).len(), 0);
}
