use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{Action, Command, HelpRow, HelpSection, ResponseTab, help_lines, update};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

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

/// hints as the footer lists them
fn listed(pairs: &[(&str, &'static str)]) -> Vec<(String, &'static str)> {
    pairs
        .iter()
        .map(|(key, label)| ((*key).to_owned(), *label))
        .collect()
}

/// the keys and labels of hints, what the footer shows of them
fn shown(hints: Vec<Hint>) -> Vec<(String, &'static str)> {
    hints.into_iter().map(|h| (h.key, h.label)).collect()
}

#[test]
fn empty_screen() {
    insta::assert_snapshot!(render(&State::default(), 80, 16));
}

#[test]
fn the_global_keys_are_always_listed_but_a_popup_takes_them() {
    let mut state = State::default();

    assert_eq!(
        shown(global_hints(&state)),
        listed(&[
            ("Ctrl+R", "send"),
            ("Ctrl+S", "save"),
            ("Ctrl+E", "environments"),
            ("Ctrl+O", "settings"),
            ("Ctrl+P", "commands"),
            ("F1", "help"),
            ("Ctrl+C", "quit"),
        ])
    );
    assert!(
        !shown(hints(&state))
            .iter()
            .any(|(key, _)| key.starts_with("Ctrl"))
    );

    update(&mut state, Action::Settings);

    assert_eq!(shown(global_hints(&state)), listed(&[("Ctrl+C", "quit")]));
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
    // 22 rows: URL bar 3, footer 2, request pane 8, response pane 9 with
    // its two borders
    assert_eq!(state.response_height.get(), 7);
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

    // on the value, Enter ends the row
    insta::assert_snapshot!(render(&state, 80, 12));
    assert_eq!(shown(hints(&state)), listed(&[("Enter", "done")]));
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
        shown(hints(&state)),
        listed(&[("Enter", "done"), ("Tab", "next field")])
    );

    update(&mut state, Action::StopEdit);

    assert!(shown(hints(&state)).contains(&("c".to_owned(), "content type")));
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
    assert!(shown(hints(&state)).contains(&("h l".to_owned(), "sideways")));
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

    pub(super) fn with_collection() -> State {
        let mut state = State::default();
        let mut workspace = sample();

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
        assert!(shown(global_hints(&state)).contains(&("Ctrl+S".to_owned(), "save")));
    }

    #[test]
    fn environment_picker() {
        let mut state = with_collection();

        update(&mut state, Action::Environments);

        insta::assert_snapshot!(render(&state, 80, 12));
        assert!(shown(hints(&state)).contains(&("v".to_owned(), "variables")));
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

mod trust {
    use std::path::PathBuf;

    use curst_core::model::{KeyValue, LocalId};

    use super::*;
    use crate::app::{Focus, Table, sample};

    #[test]
    fn the_trust_prompt_names_the_folder_and_the_choices() {
        let mut state = State::default();
        let mut shared = sample();

        shared.id = LocalId::new("c2").unwrap();
        shared.trusted = false;
        shared.root = PathBuf::from("/repo/.curst");
        shared.collection.name = "Shared".into();
        update(
            &mut state,
            Action::Loaded {
                collections: vec![shared],
                globals: vec![],
                problems: vec![],
                orphans: vec![(
                    LocalId::new("old").unwrap(),
                    PathBuf::from("/old/repo/.curst"),
                )],
            },
        );

        insta::assert_snapshot!(render(&state, 100, 18));

        // a rebound key is the one the question offers
        let bindings = vec![
            ("trust.confirm".to_owned(), vec!["t".to_owned()]),
            ("trust.trust.moved-1".to_owned(), vec!["f1".to_owned()]),
        ];

        (state.keymap, _) = crate::app::Keymap::from_settings(None, &bindings);

        let text = render(&state, 100, 18);

        assert!(text.contains(" t trust it as a new collection"), "{text}");
        assert!(text.contains(" F1 /old/repo/.curst"), "{text}");
    }

    #[test]
    fn a_credential_typed_in_a_header_is_flagged() {
        let mut state = State::default();

        state.focus = Focus::Request;
        state.request_tab = RequestTab::Headers;
        state.headers = Table::from_pairs(&[
            KeyValue::new("Accept", "*/*"),
            KeyValue::new("Authorization", "Bearer abc123"),
        ]);

        assert!(!render(&state, 80, 14).contains('⚠'));

        state.headers.selected = 1;

        insta::assert_snapshot!(render(&state, 80, 14));
        assert!(shown(hints(&state)).contains(&("m".to_owned(), "make it a secret")));

        state.headers.rows[1].value = "Bearer {{token}}".into();

        assert!(!render(&state, 80, 14).contains('⚠'));
    }
}

#[test]
fn many_tabs_keep_the_active_one_in_view() {
    let mut state = State::default();

    for i in 0..8 {
        if i > 0 {
            update(&mut state, Action::NewTab);
        }

        state
            .url
            .insert_str(format!("http://host-number-{i}.example.com/"));
    }

    update(&mut state, Action::RequestAt(3));

    let screen = render(&state, 80, 12);
    let bar = screen.lines().next().unwrap();

    assert!(bar.contains("host-number-3"), "{bar}");
    assert!(bar.contains('‹') && bar.contains('›'), "{bar}");
}

#[test]
fn the_tab_switcher_numbers_tabs_by_their_keys() {
    let mut state = State::default();

    for i in 0..10 {
        if i > 0 {
            update(&mut state, Action::NewTab);
        }

        state.url.insert_str(format!("http://host-{i}/"));
    }

    update(&mut state, Action::RequestAt(2));
    update(&mut state, Action::Tabs);

    let screen = render(&state, 80, 20);

    insta::assert_snapshot!(screen);
    assert!(shown(hints(&state)).contains(&("1-9".to_owned(), "go to")));
}

#[test]
fn hints_follow_the_keymap() {
    let bindings = vec![
        ("global.send".to_owned(), vec!["f5".to_owned()]),
        ("global.environments".to_owned(), Vec::new()),
    ];
    let (keymap, warnings) = crate::app::Keymap::from_settings(Some("vim"), &bindings);
    let mut state = State::default();

    state.keymap = keymap;

    assert_eq!(warnings, Vec::<String>::new());
    assert!(shown(global_hints(&state)).contains(&("F5".to_owned(), "send")));
    assert!(
        !shown(global_hints(&state))
            .iter()
            .any(|(_, label)| *label == "environments")
    );

    state.focus = Focus::Request;

    assert!(shown(hints(&state)).contains(&("h l".to_owned(), "tab")));
    assert!(shown(hints(&state)).contains(&("o".to_owned(), "add")));
    assert!(shown(hints(&state)).contains(&("d d".to_owned(), "delete")));
}

#[test]
fn help_lists_every_key_where_the_focus_is_first() {
    let mut state = State::default();

    state.focus = Focus::Sidebar;
    update(&mut state, Action::Help);

    let Some(Popup::Help { here, .. }) = &state.popup else {
        panic!("no help");
    };
    let sections = state.keymap.help(here);
    let names = |section: &HelpSection| -> Vec<String> {
        section.rows.iter().map(|r| r.name.clone()).collect()
    };

    assert_eq!(sections[0].title, "Here");
    assert!(names(&sections[0]).contains(&"sidebar.rename".to_owned()));
    assert!(names(&sections[0]).contains(&"panes.help".to_owned()));
    assert_eq!(sections[1].title, "Everywhere");
    assert!(sections[1].rows.contains(&HelpRow {
        keys: "Ctrl+R".into(),
        what: "send the request",
        name: "global.send".into(),
    }));
    assert!(
        sections[1]
            .rows
            .iter()
            .any(|r| r.name == "global.request.go-N" && r.keys == "Ctrl+1 … Ctrl+9")
    );
    assert!(sections.iter().any(|s| s.title == "In the URL"));
    assert!(
        sections
            .iter()
            .flat_map(|s| &s.rows)
            .all(|r| !r.what.is_empty()),
        "an action without a description"
    );

    insta::assert_snapshot!(render(&state, 90, 20));

    update(&mut state, Action::Bottom);
    update(&mut state, Action::Down);

    let Some(Popup::Help { scroll, .. }) = &state.popup else {
        panic!("no help");
    };

    assert_eq!(*scroll, help_lines(&sections) - 1);

    update(&mut state, Action::ClosePopup);

    assert!(state.popup.is_none());
}

#[test]
fn f1_opens_help_while_typing_and_question_mark_is_typed() {
    let mut state = State::default();

    for action in crate::app::action_for(&state, key(KeyCode::Char('?'))) {
        update(&mut state, action);
    }

    assert!(state.popup.is_none());
    assert_eq!(state.request().url, "?");

    for action in crate::app::action_for(&state, key(KeyCode::F(1))) {
        update(&mut state, action);
    }

    assert!(matches!(state.popup, Some(Popup::Help { .. })));
}

#[test]
fn the_footer_keeps_help_and_quit_when_narrow() {
    let text = render(&State::default(), 50, 12);
    let last = text.lines().last().unwrap_or_default().to_owned();

    assert!(last.contains("F1 help"), "{last}");
    assert!(last.contains("Ctrl+C quit"), "{last}");
    assert!(!last.contains("settings"), "{last}");
}

mod palette {
    use super::*;
    use crate::app::{Entry, sample};

    fn press(state: &mut State, event: KeyEvent) {
        for action in crate::app::action_for(state, event) {
            update(state, action);
        }
    }

    fn type_text(state: &mut State, text: &str) {
        for c in text.chars() {
            press(state, key(KeyCode::Char(c)));
        }
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn it_runs_an_action_by_a_few_of_its_letters() {
        let mut state = State::default();

        press(&mut state, ctrl('p'));

        let Some(Popup::Palette(palette)) = &state.popup else {
            panic!("no palette");
        };
        let entries = state.palette_entries(palette);

        // unbound actions are offered too, so nothing is out of reach
        assert!(entries.iter().any(
            |e| matches!(e, Entry::Run(c) if c.name == "global.settings" && c.keys.as_deref() == Some("Ctrl+O"))
        ));
        assert!(
            !entries
                .iter()
                .any(|e| matches!(e, Entry::Run(c) if c.name.split('.').next_back() == Some("up")))
        );

        type_text(&mut state, "new tab");

        insta::assert_snapshot!(render(&state, 80, 16));

        press(&mut state, key(KeyCode::Enter));

        assert!(state.popup.is_none());
        assert_eq!(state.tab_count(), 2);
    }

    #[test]
    fn it_opens_a_saved_request() {
        let mut state = State::default();

        state.collections.push(sample());
        press(&mut state, ctrl('p'));
        type_text(&mut state, "pets list");

        let Some(Popup::Palette(palette)) = &state.popup else {
            panic!("no palette");
        };

        assert!(matches!(
            state.palette_entries(palette).first(),
            Some(Entry::Open { title, .. }) if title == "Pets › Users › LIST"
        ));

        press(&mut state, key(KeyCode::Enter));

        assert_eq!(state.open.as_ref().map(|o| o.name.as_str()), Some("LIST"));
    }

    #[test]
    fn esc_closes_it_and_nothing_matching_stays_open() {
        let mut state = State::default();

        press(&mut state, ctrl('p'));
        type_text(&mut state, "zzzz");
        press(&mut state, key(KeyCode::Enter));

        assert!(matches!(state.popup, Some(Popup::Palette(_))));

        press(&mut state, key(KeyCode::Esc));

        assert!(state.popup.is_none());
        assert_eq!(state.request().url, "");
    }
}

mod themes {
    use ratatui::buffer::Buffer;
    use ratatui::style::{Color, Modifier};

    use super::*;
    use crate::theme::Theme;

    fn buffer(state: &State) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();

        terminal.draw(|frame| view(state, frame)).unwrap();

        terminal.backend().buffer().clone()
    }

    /// the first cell of the URL pane's border, which has the focus, and
    /// of the response pane's, which does not
    fn borders(buffer: &Buffer) -> (ratatui::style::Style, ratatui::style::Style) {
        let find = |text: &str| {
            let area = buffer.area;

            (0..area.height)
                .flat_map(|y| (0..area.width).map(move |x| (x, y)))
                .find(|(x, y)| {
                    (0..text.chars().count()).all(|i| {
                        let x = *x + u16::try_from(i).unwrap();

                        x < area.width
                            && buffer[(x, *y)].symbol() == text.chars().nth(i).unwrap().to_string()
                    })
                })
                .map(|(x, y)| buffer[(x - 2, y)].style())
                .unwrap()
        };

        (find("URL"), find("Response"))
    }

    #[test]
    fn every_color_comes_from_the_theme() {
        let mut state = State::default();

        let (focused, other) = borders(&buffer(&state));

        assert_eq!(focused.fg, Some(Color::Cyan));
        assert_eq!(other.fg, Some(Color::DarkGray));

        state.theme = Theme::light();

        let (focused, other) = borders(&buffer(&state));

        assert_eq!(focused.fg, Some(Color::Blue));
        assert_eq!(other.fg, Some(Color::Indexed(245)));

        state.theme = Theme::mono();

        let buffer = buffer(&state);
        let (focused, _) = borders(&buffer);

        assert!(focused.add_modifier.contains(Modifier::BOLD));
        colorless(&buffer);

        // a busier screen: a response, requests in the sidebar, a notice,
        // a credential warning and a pop-up
        let (mut state, id) = sent_state("http://localhost/pets");

        update(
            &mut state,
            Action::responded(id, Ok(crate::app::tests::response(r#"{"id":1}"#))),
        );
        state.theme = Theme::mono();
        state.collections.push(crate::app::sample());
        state
            .sidebar
            .expanded
            .insert(crate::app::NodeKey::root(crate::app::sample().id));
        state.notice = Some("a notice".into());
        state.headers = crate::app::Table::from_pairs(&[curst_core::model::KeyValue::new(
            "Authorization",
            "Bearer abcdefghijklmnopqrstuvwxyz0123456789",
        )]);
        state.focus = Focus::Request;
        state.request_tab = RequestTab::Headers;

        colorless(&self::buffer(&state));

        update(&mut state, Action::Palette);

        colorless(&self::buffer(&state));

        update(&mut state, Action::ClosePopup);
        update(&mut state, Action::Settings);

        colorless(&self::buffer(&state));
    }

    /// nothing on the screen has a color
    fn colorless(buffer: &Buffer) {
        let area = buffer.area;

        for y in 0..area.height {
            for x in 0..area.width {
                let style = buffer[(x, y)].style();

                assert!(
                    matches!(style.fg, None | Some(Color::Reset))
                        && matches!(style.bg, None | Some(Color::Reset)),
                    "a color at {x},{y}: {style:?}"
                );
            }
        }
    }
}

mod preferences {
    use curst_core::model::LocalId;

    use super::*;
    use crate::app::Step;
    use crate::theme::Theme;

    fn saved(commands: &[Command]) -> Option<(Option<&str>, Option<&str>)> {
        commands.iter().find_map(|c| match c {
            Command::SavePreferences { keys, theme } => Some((keys.as_deref(), theme.as_deref())),
            _ => None,
        })
    }

    #[test]
    fn the_welcome_tries_each_choice_then_saves_and_gives_way() {
        let mut state = State::default();
        let collection = LocalId::new("c1").unwrap();

        state.popup = Some(Popup::Trust {
            collection: collection.clone(),
        });
        update(&mut state, Action::Welcome);

        insta::assert_snapshot!(render(&state, 90, 20));

        update(&mut state, Action::Down);

        assert_eq!(state.keys_preset, "vim");
        assert!(shown(hints(&state)).contains(&("↑↓".to_owned(), "choose")));

        update(&mut state, Action::ConfirmPopup);

        assert!(matches!(
            state.popup,
            Some(Popup::Welcome {
                step: Step::Theme,
                ..
            })
        ));

        update(&mut state, Action::Down);

        assert_eq!(state.theme, Theme::light());

        let commands = update(&mut state, Action::ConfirmPopup);

        assert_eq!(saved(&commands), Some((Some("vim"), Some("light"))));
        assert!(matches!(&state.popup, Some(Popup::Trust { collection: c }) if *c == collection));
    }

    #[test]
    fn esc_in_the_welcome_keeps_the_defaults_and_saves_them() {
        let mut state = State::default();

        update(&mut state, Action::Welcome);

        let commands = update(&mut state, Action::ClosePopup);

        assert_eq!(saved(&commands), Some((Some("standard"), Some("dark"))));
        assert!(state.popup.is_none());
    }

    #[test]
    fn the_settings_switch_preset_and_theme_and_keep_own_bindings() {
        let mut state = State::default();

        state.key_bindings = vec![("global.send".into(), vec!["f5".into()])];
        update(&mut state, Action::Settings);

        insta::assert_snapshot!(render(&state, 90, 16));

        let commands = update(&mut state, Action::NextKeys);

        assert_eq!(saved(&commands), Some((Some("vim"), None)));
        assert_eq!(state.key(crate::app::Layer::Global, "send"), "F5");
        assert_eq!(state.key(crate::app::Layer::Sidebar, "top"), "g g");

        let commands = update(&mut state, Action::NextTheme);

        assert_eq!(saved(&commands), Some((None, Some("light"))));
        assert!(matches!(state.popup, Some(Popup::Settings)));

        update(&mut state, Action::NextTheme);
        update(&mut state, Action::NextTheme);

        assert_eq!(state.theme, Theme::dark());
    }
}

#[test]
fn a_half_typed_sequence_shows_in_the_footer() {
    let (keymap, _) = crate::app::Keymap::from_settings(Some("vim"), &[]);
    let mut state = State::default();

    state.keymap = keymap;
    state.focus = Focus::Sidebar;

    for action in crate::app::action_for(&state, key(KeyCode::Char('g'))) {
        update(&mut state, action);
    }

    let text = render(&state, 80, 12);

    assert!(text.contains(" g … "), "{text}");
}

#[test]
fn shifted_symbols_work_however_the_terminal_reports_shift() {
    let (keymap, _) = crate::app::Keymap::from_settings(Some("vim"), &[]);

    for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
        let mut state = State::default();

        state.keymap = keymap.clone();
        state.focus = Focus::Sidebar;

        let help = crate::app::action_for(&state, KeyEvent::new(KeyCode::Char('?'), modifiers));
        let palette = crate::app::action_for(&state, KeyEvent::new(KeyCode::Char(':'), modifiers));

        assert!(matches!(help.as_slice(), [Action::Help]), "{help:?}");
        assert!(
            matches!(palette.as_slice(), [Action::Palette]),
            "{palette:?}"
        );
    }
}

mod mouse {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    use super::*;
    use crate::app::{Focus, Popup, mouse_actions, mouse_acts};

    const WIDTH: u16 = 120;
    const HEIGHT: u16 = 24;

    fn event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// draws the screen, as the runtime does before a click, and finds
    /// where `text` is on it
    fn find(state: &State, text: &str) -> (u16, u16) {
        let screen = render(state, WIDTH, HEIGHT);
        let lines: Vec<&str> = screen.lines().collect();

        for (y, line) in lines.iter().enumerate() {
            // each line is quoted in the rendered text
            let line: Vec<char> = line.chars().skip(1).collect();
            let wanted: Vec<char> = text.chars().collect();

            if let Some(x) = line.windows(wanted.len()).position(|w| w == wanted) {
                return (u16::try_from(x).unwrap(), u16::try_from(y).unwrap());
            }
        }

        panic!("{text:?} is not on the screen:\n{screen}");
    }

    /// draws, then sends what a mouse event at `(x, y)` gives
    fn send(state: &mut State, kind: MouseEventKind, (x, y): (u16, u16)) -> Vec<Command> {
        render(state, WIDTH, HEIGHT);

        let mut commands = Vec::new();

        for action in mouse_actions(state, event(kind, x, y)) {
            commands.extend(update(state, action));
        }

        commands
    }

    fn click(state: &mut State, at: (u16, u16)) -> Vec<Command> {
        send(state, MouseEventKind::Down(MouseButton::Left), at)
    }

    fn click_on(state: &mut State, text: &str) -> Vec<Command> {
        let at = find(state, text);

        click(state, at)
    }

    #[test]
    fn only_a_press_and_the_wheel_do_anything() {
        let mut state = State::default();

        render(&state, WIDTH, HEIGHT);

        let at = find(&state, "Response");

        for kind in [
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Moved,
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Down(MouseButton::Right),
        ] {
            assert!(!mouse_acts(event(kind, at.0, at.1)));
            assert_eq!(mouse_actions(&state, event(kind, at.0, at.1)).len(), 0);
        }

        assert!(mouse_acts(event(MouseEventKind::ScrollDown, 0, 0)));

        // a press focuses once, and its release changes nothing more
        click(&mut state, at);

        assert_eq!(state.focus, Focus::Response);
        assert_eq!(state.response_tab, ResponseTab::Body);
    }

    #[test]
    fn panes_tabs_and_the_method_answer_clicks() {
        let mut state = State::default();

        click_on(&mut state, "Headers");

        assert_eq!(state.focus, Focus::Request);
        assert_eq!(state.request_tab, RequestTab::Headers);

        click_on(&mut state, "Timing");

        assert_eq!(state.focus, Focus::Response);
        assert_eq!(state.response_tab, ResponseTab::Timing);

        let (x, y) = find(&state, "Method");

        click(&mut state, (x, y + 1));

        assert_eq!(state.focus, Focus::Method);
        assert_eq!(state.method.as_str(), "POST");

        // the first tab is the one whose method was just changed
        update(&mut state, Action::NewTab);
        click_on(&mut state, "POST new request");

        assert_eq!(state.active_tab, 0);
    }

    #[test]
    fn a_click_in_the_url_puts_the_cursor_there() {
        let mut state = State::default();

        state.url.insert_str("https://example.com/pets");
        state.focus = Focus::Sidebar;

        // the tab bar names it too, without the scheme
        let (x, y) = find(&state, "https://example.com");

        click(&mut state, (x + 11, y));

        assert_eq!(state.focus, Focus::Url);
        assert_eq!(state.url.cursor(), (0, "https://exa".len()));
    }

    #[test]
    fn sidebar_rows_open_requests_and_fold_folders() {
        let mut state = super::collections::with_collection();

        state.focus = Focus::Url;
        click_on(&mut state, "CREATE");

        assert_eq!(state.focus, Focus::Sidebar);
        assert_eq!(state.open.as_ref().map(|o| o.name.as_str()), Some("CREATE"));

        click_on(&mut state, "Users");
        click_on(&mut state, "LIST");

        assert_eq!(state.open.as_ref().map(|o| o.name.as_str()), Some("LIST"));
    }

    #[test]
    fn table_rows_select_then_edit_and_the_box_turns_them_off() {
        let mut state = State::default();

        state.request_tab = RequestTab::Headers;
        state.headers = crate::app::Table::from_pairs(&[
            curst_core::model::KeyValue::new("Accept", "*/*"),
            curst_core::model::KeyValue::new("X-Trace", "on"),
        ]);

        click_on(&mut state, "X-Trace");

        assert_eq!(state.focus, Focus::Request);
        assert_eq!(state.headers.selected, 1);
        assert!(!state.request_typing());

        click_on(&mut state, "X-Trace");

        assert!(state.request_typing());

        click_on(&mut state, "[x] ");

        assert!(!state.request_typing());
        assert_eq!(state.headers.selected, 0);
        assert!(!state.headers.rows[0].enabled);
    }

    #[test]
    fn footer_keys_are_buttons() {
        let mut state = State::default();

        click_on(&mut state, "F1 help");

        assert!(matches!(state.popup, Some(Popup::Help { .. })));

        click_on(&mut state, "Esc close");

        assert!(state.popup.is_none());
    }

    #[test]
    fn a_popup_takes_the_clicks_and_some_close_from_outside() {
        let mut state = State::default();

        update(&mut state, Action::Settings);

        // inside it, nothing under it is reached
        let (x, y) = find(&state, "Keys");

        click(&mut state, (x + 30, y + 3));

        assert!(matches!(state.popup, Some(Popup::Settings)));

        let commands = click_on(&mut state, "next preset");

        assert_eq!(state.keys_preset, "vim");
        assert!(matches!(
            commands.as_slice(),
            [Command::SavePreferences { .. }]
        ));

        click(&mut state, (1, 1));

        assert!(state.popup.is_none());
        assert_eq!(state.focus, Focus::Url);

        // a question is only answered on purpose
        state.popup = Some(Popup::confirm("Really?", crate::app::Pending::CloseTab));
        click(&mut state, (1, 1));

        assert!(matches!(state.popup, Some(Popup::Confirm { .. })));
    }

    #[test]
    fn closing_changed_variables_by_a_click_still_asks() {
        let mut state = super::collections::with_collection();

        update(&mut state, Action::Environments);
        click_on(&mut state, "Dev");

        assert!(state.popup.is_none(), "a click on an environment uses it");

        update(&mut state, Action::Environments);
        update(&mut state, Action::Down);
        update(&mut state, Action::EditVariables);
        update(&mut state, Action::DeleteRow);
        click(&mut state, (WIDTH - 1, 0));

        assert!(matches!(state.popup, Some(Popup::Confirm { .. })));
    }

    #[test]
    fn list_popups_pick_by_click() {
        let mut state = State::default();

        update(&mut state, Action::NewTab);
        update(&mut state, Action::Tabs);

        let (x, y) = find(&state, "• GET");

        click(&mut state, (x, y - 1));

        assert_eq!(state.active_tab, 0);
        assert!(state.popup.is_none());

        update(&mut state, Action::Palette);

        for c in "settings".chars() {
            update(&mut state, Action::Type(key(KeyCode::Char(c))));
        }

        // the entry, not the query typed above it
        click_on(&mut state, " settings ");

        assert!(matches!(state.popup, Some(Popup::Settings)));

        state.popup = None;
        update(&mut state, Action::Welcome);
        click_on(&mut state, "vim ");

        assert_eq!(state.keys_preset, "vim");
        assert!(matches!(state.popup, Some(Popup::Welcome { .. })));
    }

    #[test]
    fn the_wheel_scrolls_what_is_under_it() {
        let (mut state, id) = sent_state("http://localhost/");
        let body: String = (0..100)
            .map(|n| ["line ", &n.to_string(), "\n"].concat())
            .collect();

        update(
            &mut state,
            Action::responded(id, Ok(crate::app::tests::response(&body))),
        );
        state.focus = Focus::Url;

        let at = find(&state, "line 0");

        send(&mut state, MouseEventKind::ScrollDown, at);

        assert_eq!(state.scroll, 3);
        assert_eq!(
            state.focus,
            Focus::Url,
            "the response scrolls without the focus"
        );

        send(&mut state, MouseEventKind::ScrollUp, at);

        assert_eq!(state.scroll, 0);

        // a field being typed in keeps the focus
        state.request_tab = RequestTab::Headers;
        state.focus = Focus::Request;
        update(&mut state, Action::AddRow);

        let at = find(&state, "Request");

        assert!(state.request_typing());
        assert_eq!(
            mouse_actions(&state, event(MouseEventKind::ScrollDown, at.0, at.1 + 5)).len(),
            0
        );
    }

    #[test]
    fn a_click_in_the_body_starts_writing_there() {
        let mut state = State::default();

        state.request_tab = RequestTab::Body;
        update(&mut state, Action::CycleKind);
        state.body.text = crate::app::body_text("{\n  \"id\": 1\n}");

        let (x, y) = find(&state, "\"id\"");

        click(&mut state, (x + 2, y));

        assert_eq!(state.focus, Focus::Request);
        assert!(state.request_typing());
        assert_eq!(state.body.text.cursor(), (1, 4));
    }

    #[test]
    fn the_trust_question_answers_by_click() {
        let mut state = State::default();
        let mut shared = crate::app::sample();

        shared.trusted = false;
        update(
            &mut state,
            Action::Loaded {
                collections: vec![shared],
                globals: vec![],
                problems: vec![],
                orphans: vec![],
            },
        );

        assert!(matches!(state.popup, Some(Popup::Trust { .. })));

        click_on(&mut state, "not now");

        assert!(state.popup.is_none());
    }

    #[test]
    fn a_narrow_screen_has_no_sidebar_to_click() {
        let mut state = super::collections::with_collection();

        state.focus = Focus::Url;
        render(&state, 50, HEIGHT);

        for action in mouse_actions(&state, event(MouseEventKind::Down(MouseButton::Left), 1, 8)) {
            update(&mut state, action);
        }

        assert_ne!(state.focus, Focus::Sidebar);
    }
}

#[test]
fn a_new_row_shows_it_wants_a_name_and_a_value() {
    let mut state = State::default();

    state.focus = Focus::Request;
    state.request_tab = RequestTab::Headers;
    state.headers =
        crate::app::Table::from_pairs(&[curst_core::model::KeyValue::new("Accept", "")]);
    update(&mut state, Action::AddRow);

    insta::assert_snapshot!(render(&state, 100, 18));
    assert_eq!(
        shown(hints(&state)),
        listed(&[("Enter", "to the value"), ("Esc", "done")])
    );

    for c in "X-Trace".chars() {
        update(&mut state, Action::Type(key(KeyCode::Char(c))));
    }

    // Enter goes on to the value, and from the value ends the row
    for action in crate::app::action_for(&state, key(KeyCode::Enter)) {
        update(&mut state, action);
    }

    assert_eq!(state.editing_column(), Some(crate::app::Column::Value));
    assert_eq!(shown(hints(&state)), listed(&[("Enter", "done")]));

    for action in crate::app::action_for(&state, key(KeyCode::Enter)) {
        update(&mut state, action);
    }

    assert_eq!(state.editing_column(), None);
    assert_eq!(state.headers.rows[1].name, "X-Trace");
}
