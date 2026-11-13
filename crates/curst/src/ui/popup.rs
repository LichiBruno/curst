use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use super::request::table;
use super::{field, pane, tab_regions, tab_titles};
use curst_core::model::LocalId;
use curst_core::secrets::{Approval, SecretKey};
use curst_core::store::Backend;

use ratatui_textarea::TextArea;

use unicode_width::UnicodeWidthStr;

use crate::app::{
    Entry, Field, Hint, Hints, Layer, PRESETS, Palette, Popup, State, Step, Target, VarTab,
    VariablesEditor, display,
};

/// a box of at most `width` by `height` in the middle of the screen
fn centered(frame: &Frame, width: u16, height: u16) -> Rect {
    let [area] = Layout::vertical([Constraint::Max(height)])
        .flex(Flex::Center)
        .areas(frame.area());
    let [area] = Layout::horizontal([Constraint::Max(width)])
        .flex(Flex::Center)
        .areas(area);

    area
}

pub(super) fn draw(state: &State, popup: &Popup, frame: &mut Frame) {
    let theme = &state.theme;

    match popup {
        Popup::Prompt { purpose, input } => {
            let area = centered(frame, 60, 3);
            let block = pane(theme, Line::from(format!(" {} ", purpose.title())), true);
            let inner = block.inner(area);

            frame.render_widget(Clear, area);
            state.region(area, Target::Popup);
            frame.render_widget(block, area);
            state.region(inner, Target::Text(Field::Popup));
            frame.render_widget(&*field(&state.theme, input, None), inner);
        }
        Popup::Confirm { message, .. } => {
            let area = centered(frame, 60, 5);
            let block = pane(theme, Line::from(" Are you sure? "), true);
            let text = vec![
                Line::from(display(message)),
                Line::from(""),
                Line::from(vec![
                    key_span(state, Layer::Confirm, "confirm"),
                    Span::raw("yes   "),
                    key_span(state, Layer::Confirm, "close"),
                    Span::raw("no"),
                ]),
            ];

            frame.render_widget(Clear, area);
            state.region(area, Target::Popup);
            frame.render_widget(
                Paragraph::new(text).wrap(Wrap { trim: true }).block(block),
                area,
            );
        }
        Popup::Environments {
            collection,
            selected,
        } => {
            let Some(workspace) = state.workspace(collection) else {
                return;
            };

            let environments = &workspace.collection.environments;
            let height = u16::try_from(environments.len() + 3).unwrap_or(u16::MAX);
            let area = centered(frame, 44, height);
            let title = format!(" Environments · {} ", display(&workspace.collection.name));
            let block = pane(theme, Line::from(title), true);
            let names = std::iter::once((None, "No environment".to_owned()))
                .chain(environments.iter().map(|e| (Some(&e.id), display(&e.name))));
            let mut lines: Vec<Line> = names
                .enumerate()
                .map(|(index, (id, name))| {
                    let active = id == workspace.active_environment.as_ref();
                    let mark = if active { "✓ " } else { "  " };
                    let style = if index == *selected {
                        theme.selected
                    } else {
                        Style::default()
                    };

                    Line::from(Span::styled(format!("{mark}{name}"), style))
                })
                .collect();

            if environments.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!(
                        "  press {} to add one",
                        state.key(Layer::Environments, "env.new")
                    ),
                    theme.muted,
                )));
            }

            frame.render_widget(Clear, area);
            state.region(area, Target::Popup);

            for index in 0..=environments.len() {
                line_region(state, block.inner(area), index, Target::Item(index));
            }

            frame.render_widget(Paragraph::new(lines).block(block), area);
        }
        Popup::Trust { collection } => trust(state, collection, frame),
        Popup::ChooseBackend => choose_backend(state, frame),
        Popup::Passphrase {
            new,
            first,
            input,
            error,
            ..
        } => passphrase(state, *new, first.is_some(), input, error.as_deref(), frame),
        Popup::Settings => settings(state, frame),
        Popup::Welcome { step, .. } => welcome(state, *step, frame),
        Popup::Tabs { selected } => tab_list(state, *selected, frame),
        Popup::Help { scroll, here } => help(state, *scroll, here, frame),
        Popup::Palette(palette) => palette_list(state, palette, frame),
        Popup::Approve(approval) => approve(state, approval, frame),
        Popup::Variables(editor) => variables(state, editor, frame),
    }
}

fn variables(state: &State, editor: &VariablesEditor, frame: &mut Frame) {
    let theme = &state.theme;
    let area = centered(frame, 100, frame.area().height.saturating_sub(4).max(8));
    let label = |tab: VarTab| match (tab, &editor.environment) {
        (VarTab::Environment, Some(_)) => "Environment",
        (tab, _) => tab.label(),
    };
    let mut title = vec![Span::raw(" Variables")];

    title.extend(tab_titles(theme, &editor.tabs(), editor.tab, label));

    let tabs = title.clone();

    if let (VarTab::Environment, Some(environment)) = (editor.tab, &editor.environment) {
        title.push(Span::raw(format!("· {} ", display(&environment.name))));
    }

    let block = pane(theme, Line::from(title), true);
    let inner = block.inner(area);
    let add = state.key(Layer::Variables, "row.add");
    let empty = match editor.tab {
        VarTab::Environment => {
            format!("no variables in this environment, press {add} to add one")
        }
        VarTab::Collection => format!("no collection variables, press {add} to add one"),
        VarTab::Globals => format!("no global variables, press {add} to add one"),
    };

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);
    tab_regions(state, area, &tabs, 2, Target::PopupTab);
    frame.render_widget(block, area);
    table(
        state,
        editor.table(),
        ["Variable", "Value"],
        true,
        &empty,
        frame,
        inner,
    );
}

/// the open tabs, numbered as the keys that reach them: 1 to 8 for the
/// first eight, and 9 for the last
fn tab_list(state: &State, selected: usize, frame: &mut Frame) {
    let theme = &state.theme;
    let labels = state.tab_labels();
    let last = labels.len() - 1;
    let dim = theme.muted;
    let key = |name: &str| state.keymap.key_for(Layer::Tabs, name);
    let numbers: Vec<String> = (0..labels.len())
        .map(|index| {
            let name = match index {
                _ if index == last && index >= 8 => "request.go-last".to_owned(),
                0..8 => format!("request.go-{}", index + 1),
                _ => return String::new(),
            };

            key(&name).unwrap_or_default()
        })
        .collect();
    let width = numbers.iter().map(|n| n.width()).max().unwrap_or(0);
    let lines: Vec<Line> = labels
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let number = &numbers[index];
            let number = format!("{number}{}", " ".repeat(width - number.width()));
            let current = if label.active { "•" } else { " " };
            let dirty = if label.dirty { " ●" } else { "" };
            let text = format!(
                " {number} {current} {:<7} {}{dirty} ",
                label.method.as_str(),
                display(&label.name)
            );
            let style = if index == selected {
                theme.selected
            } else {
                Style::default()
            };

            Line::from(vec![Span::styled(text, style)])
        })
        .chain(std::iter::once(Line::from(Span::styled(
            " • is the tab you are on",
            dim,
        ))))
        .collect();
    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let area = centered(frame, 60, height);

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);

    for index in 0..labels.len() {
        line_region(
            state,
            area.inner(Margin::new(1, 1)),
            index,
            Target::Item(index),
        );
    }

    frame.render_widget(
        Paragraph::new(lines).block(pane(theme, Line::from(" Tabs "), true)),
        area,
    );
}

/// the palette: the query, then what matches it, best first
fn palette_list(state: &State, palette: &Palette, frame: &mut Frame) {
    const SHOWN: usize = 12;

    let theme = &state.theme;
    let entries = state.palette_entries(palette);
    let dim = theme.muted;
    let rows = entries.len().clamp(1, SHOWN);
    let height = u16::try_from(rows + 3).unwrap_or(u16::MAX);
    let area = centered(frame, 72, height);
    let block = pane(theme, Line::from(" Commands and requests "), true);
    let inner = block.inner(area);
    let [input_area, list_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
    let width = usize::from(list_area.width);
    let first = palette.selected.saturating_sub(SHOWN - 1);
    let mut lines: Vec<Line> = Vec::new();

    for (index, entry) in entries.iter().enumerate().skip(first).take(SHOWN) {
        let (left, right) = match entry {
            Entry::Run(command) => (
                format!(" {}", command.what),
                format!("{} ", command.keys.as_deref().unwrap_or(&command.name)),
            ),
            Entry::Open { method, title, .. } => (
                format!(" {:<7} {}", method.as_str(), display(title)),
                "open ".to_owned(),
            ),
        };
        let gap = width.saturating_sub(left.width() + right.width()).max(1);
        let style = if index == palette.selected {
            theme.selected
        } else {
            Style::default()
        };

        lines.push(Line::from(vec![
            Span::styled(format!("{left}{}", " ".repeat(gap)), style),
            Span::styled(right, style.patch(dim)),
        ]));
    }

    if entries.is_empty() {
        lines.push(Line::from(Span::styled(" nothing matches", dim)));
    }

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);
    frame.render_widget(block, area);
    state.region(input_area, Target::Text(Field::Popup));

    for index in first..entries.len().min(first + SHOWN) {
        line_region(state, list_area, index - first, Target::Item(index));
    }

    frame.render_widget(&*field(&state.theme, &palette.input, None), input_area);
    frame.render_widget(Paragraph::new(lines), list_area);
}

fn help(state: &State, scroll: usize, here: &[Layer], frame: &mut Frame) {
    let theme = &state.theme;
    let sections = state.keymap.help(here);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = theme.muted;
    let width = sections
        .iter()
        .flat_map(|s| &s.rows)
        .map(|r| r.keys.width())
        .max()
        .unwrap_or(0)
        .min(24);
    let what = sections
        .iter()
        .flat_map(|s| &s.rows)
        .map(|r| r.what.width())
        .max()
        .unwrap_or(0)
        .min(40);
    let mut lines = Vec::new();

    for section in &sections {
        lines.push(Line::from(Span::styled(
            format!(" {}", section.title),
            bold,
        )));

        for row in &section.rows {
            let pad = width.saturating_sub(row.keys.width());

            lines.push(Line::from(vec![
                Span::styled(format!("   {}{} ", row.keys, " ".repeat(pad)), bold),
                Span::raw(format!(
                    " {}{}  ",
                    row.what,
                    " ".repeat(what.saturating_sub(row.what.width()))
                )),
                Span::styled(row.name.clone(), dim),
            ]));
        }

        lines.push(Line::default());
    }

    let height = frame.area().height.saturating_sub(4).max(8);
    let area = centered(frame, 90, height);
    let visible = usize::from(height.saturating_sub(2));
    let scroll = scroll.min(lines.len().saturating_sub(visible));
    let title = Line::from(" Keys · rebind them in config.toml, [keys.bindings] ");

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0))
            .block(pane(theme, title, true)),
        area,
    );
}

/// records line `row` of `inner` as `target`, for clicks
fn line_region(state: &State, inner: Rect, row: usize, target: Target) {
    let Ok(row) = u16::try_from(row) else {
        return;
    };

    let line = Rect {
        y: inner.y.saturating_add(row),
        height: 1,
        ..inner
    };

    state.region(line.intersection(inner), target);
}

/// records span `index` of `line`, a key, and the span after it, what it
/// does, drawn on line `row` of `inner` without wrapping, as a button for
/// the action `name`
fn button(state: &State, inner: Rect, row: usize, line: &Line, index: usize, name: &str) {
    let x: usize = line.spans.iter().take(index).map(Span::width).sum();
    let width: usize = line.spans.iter().skip(index).take(2).map(Span::width).sum();
    let (Ok(x), Ok(row), Ok(width)) = (u16::try_from(x), u16::try_from(row), u16::try_from(width))
    else {
        return;
    };

    let span = Rect::new(
        inner.x.saturating_add(x),
        inner.y.saturating_add(row),
        width,
        1,
    );

    state.region(span.intersection(inner), Target::Run(name.to_owned()));
}

/// a key, padded, as the pop-ups list them
fn key_span(state: &State, layer: Layer, name: &str) -> Span<'static> {
    Span::styled(format!(" {} ", state.key(layer, name)), state.theme.key)
}

fn settings(state: &State, frame: &mut Frame) {
    let theme = &state.theme;
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let (now, action) = match state.backend {
        None => ("not chosen yet", "choose where to keep them"),
        Some(Backend::Keychain) => ("the system keychain", "move them to an encrypted file"),
        Some(Backend::EncryptedFile) => (
            "an encrypted file, secrets.age",
            "move them to the keychain",
        ),
    };
    let row = |label: &'static str, value: String, name: &str, what: &'static str| {
        Line::from(vec![
            Span::raw(format!("{label:<9}")),
            Span::styled(format!("{value:<16}"), bold),
            key_span(state, Layer::Settings, name),
            Span::styled(what, theme.muted),
        ])
    };
    let lines = vec![
        row("Keys", state.keys_preset.clone(), "keys", "next preset"),
        row("Theme", state.theme.name.clone(), "theme", "next theme"),
        Line::from(""),
        Line::from(vec![
            Span::raw(format!("{:<9}", "Secrets")),
            Span::styled(now, bold),
        ]),
        Line::from(vec![
            Span::raw(" ".repeat(9)),
            key_span(state, Layer::Settings, "migrate"),
            Span::raw(action),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Changes are saved to config.toml at once. Moving secrets copies each one,",
            theme.muted,
        )),
        Line::from(Span::styled(
            "checks the copy, then deletes the old one.",
            theme.muted,
        )),
    ];
    let area = centered(frame, 80, 10);
    let inner = area.inner(Margin::new(1, 1));

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);

    // each setting's line changes it
    for (row, name) in [(0, "keys"), (1, "theme"), (4, "migrate")] {
        line_region(state, inner, row, Target::Run(name.to_owned()));
    }

    frame.render_widget(
        Paragraph::new(lines).block(pane(theme, Line::from(" Settings "), true)),
        area,
    );
}

/// the first run: how keys work, then how it looks, each tried at once
fn welcome(state: &State, step: Step, frame: &mut Frame) {
    let theme = &state.theme;
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let choice = |chosen: bool, name: &str, what: &str| {
        let mark = if chosen { "›" } else { " " };
        let style = if chosen {
            theme.selected
        } else {
            Style::default()
        };

        Line::from(vec![
            Span::raw(format!("  {mark} ")),
            Span::styled(format!(" {name:<10}"), style),
            Span::styled(format!("  {what}"), theme.muted),
        ])
    };
    let mut lines = vec![
        Line::from(Span::styled("Welcome to curst", bold)),
        Line::from(""),
    ];

    match step {
        Step::Keys => {
            lines.push(Line::from("How do you like to move around?"));
            lines.push(Line::from(""));

            for (name, what) in PRESETS {
                lines.push(choice(state.keys_preset == name, name, what));
            }
        }
        Step::Theme => {
            lines.push(Line::from(
                "Which colors suit your terminal? It changes as you choose.",
            ));
            lines.push(Line::from(""));

            for option in &state.themes {
                let what = match option.name.as_str() {
                    "dark" => "for a dark background",
                    "light" => "for a light background",
                    "mono" => "no color at all",
                    _ => "yours, from the themes folder",
                };

                lines.push(choice(state.theme.name == option.name, &option.name, what));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Both can be changed later in the settings, and every key in config.toml.",
        theme.muted,
    )));

    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let area = centered(frame, 78, height);
    let title = match step {
        Step::Keys => " Keys · 1 of 2 ",
        Step::Theme => " Theme · 2 of 2 ",
    };
    let choices = match step {
        Step::Keys => PRESETS.len(),
        Step::Theme => state.themes.len(),
    };

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);

    // the choices come after the greeting and the question
    for index in 0..choices {
        line_region(
            state,
            area.inner(Margin::new(1, 1)),
            4 + index,
            Target::Item(index),
        );
    }

    frame.render_widget(
        Paragraph::new(lines).block(pane(theme, Line::from(title), true)),
        area,
    );
}

fn choose_backend(state: &State, frame: &mut Frame) {
    let theme = &state.theme;
    let lines = vec![
        Line::from("This request uses a secret. Where should curst keep secrets?"),
        Line::from(""),
        Line::from(vec![
            key_span(state, Layer::Backend, "backend.keychain"),
            Span::raw("the system keychain: nothing to type, but it stays on"),
        ]),
        Line::from("    this machine, and some Linux setups have none"),
        Line::from(vec![
            key_span(state, Layer::Backend, "backend.file"),
            Span::raw("an encrypted file: works everywhere, asks for a"),
        ]),
        Line::from("    passphrase once per session"),
        Line::from(""),
        Line::from(Span::styled(
            "You can switch later in Settings.",
            theme.muted,
        )),
    ];
    let area = centered(frame, 68, 10);
    let inner = area.inner(Margin::new(1, 1));

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);

    // each choice takes two lines
    for (row, name) in [(2, "backend.keychain"), (4, "backend.file")] {
        line_region(state, inner, row, Target::Run(name.to_owned()));
        line_region(state, inner, row + 1, Target::Run(name.to_owned()));
    }

    frame.render_widget(
        Paragraph::new(lines).block(pane(theme, Line::from(" Keep secrets where? "), true)),
        area,
    );
}

fn passphrase(
    state: &State,
    new: bool,
    confirming: bool,
    input: &TextArea<'static>,
    error: Option<&str>,
    frame: &mut Frame,
) {
    let title = match (new, confirming) {
        (true, false) => " Choose a passphrase for secrets.age ",
        (true, true) => " Type the passphrase again ",
        (false, _) => " Passphrase for secrets.age ",
    };
    let theme = &state.theme;
    let area = centered(frame, 60, 4);
    let block = pane(theme, Line::from(title), true);
    let inner = block.inner(area);
    let [field, message] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);
    frame.render_widget(block, area);
    frame.render_widget(&*super::field(theme, input, None), field);

    if let Some(error) = error {
        frame.render_widget(
            Paragraph::new(Span::styled(error.to_owned(), theme.warning)),
            message,
        );
    }
}

fn approve(state: &State, approval: &Approval, frame: &mut Frame) {
    let theme = &state.theme;

    let bold = Style::default().add_modifier(Modifier::BOLD);
    let names: Vec<String> = approval
        .keys
        .iter()
        .map(|key| {
            let scope = match key {
                SecretKey::Environment { .. } => "environment",
                SecretKey::Collection { .. } => "collection",
                SecretKey::Global { .. } | SecretKey::Probe => "global",
            };

            format!(
                "{} ({scope})",
                key.name().map_or_else(String::new, ToString::to_string)
            )
        })
        .collect();
    let mut lines = vec![
        Line::from("This request would send these secrets for the first time to"),
        Line::from(Span::styled(display(&approval.origin), bold)),
        Line::from(""),
        Line::from(display(&names.join(", "))),
    ];

    if approval.origin.starts_with("http://") {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "⚠ plain http: anyone on the network path can read them",
            theme.warning,
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        key_span(state, Layer::Approve, "confirm"),
        Span::raw("send them there, now and later   "),
        key_span(state, Layer::Approve, "close"),
        Span::raw("don't send"),
    ]));

    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let area = centered(frame, 70, height);

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(pane(
            theme,
            Line::from(" Send secrets? "),
            true,
        )),
        area,
    );
}

fn trust(state: &State, collection: &LocalId, frame: &mut Frame) {
    let theme = &state.theme;
    let Some(workspace) = state.workspace(collection) else {
        return;
    };

    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = theme.muted;
    let path = std::fs::canonicalize(&workspace.root).unwrap_or_else(|_| workspace.root.clone());
    let mut lines = vec![
        Line::from(Span::styled(display(&path.display().to_string()), bold)),
        Line::from(Span::styled(
            format!("calls itself {}", display(&workspace.collection.name)),
            dim,
        )),
        Line::from(""),
        Line::from("Requests from this folder are not sent and no secret is used"),
        Line::from("until you trust it. Trust it only if you trust where it came from."),
        Line::from(""),
        Line::from(vec![
            key_span(state, Layer::Trust, "confirm"),
            Span::raw("trust it as a new collection   "),
            key_span(state, Layer::Trust, "close"),
            Span::raw("not now, only look"),
        ]),
    ];

    if !state.orphans.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(
            "or, if it was moved from one of these folders, keep its old secrets:",
        ));

        for (index, (_, old)) in state.orphans.iter().take(9).enumerate() {
            let key = state
                .keymap
                .key_for(Layer::Trust, &format!("trust.moved-{}", index + 1))
                .unwrap_or_default();

            lines.push(Line::from(vec![
                Span::styled(format!(" {key} "), bold),
                Span::raw(display(&old.display().to_string())),
            ]));
        }
    }

    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let area = centered(frame, 76, height);
    let block = pane(theme, Line::from(" Trust this folder? "), true);
    let inner = block.inner(area);

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);

    // the answers, then the folders it may have been moved from
    if let Some(answers) = lines.get(6) {
        button(state, inner, 6, answers, 0, "confirm");
        button(state, inner, 6, answers, 2, "close");
    }

    for index in 0..state.orphans.len().min(9) {
        let name = format!("trust.moved-{}", index + 1);

        line_region(state, inner, 9 + index, Target::Run(name));
    }

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

pub(super) fn hints(state: &State, popup: &Popup) -> Vec<Hint> {
    let mut hints = Hints::here(state);

    match popup {
        Popup::Prompt { .. } | Popup::Passphrase { .. } => {
            hints.one("confirm", "ok").one("close", "cancel");
        }
        Popup::Confirm { .. } => {
            hints.one("confirm", "yes").one("close", "no");
        }
        Popup::Trust { .. } => {
            hints.one("confirm", "trust").one("close", "not now").range(
                "trust.moved-1",
                "trust.moved-9",
                "moved from",
            );
        }
        Popup::ChooseBackend => {
            hints
                .one("backend.keychain", "keychain")
                .one("backend.file", "encrypted file")
                .one("close", "later");
        }
        Popup::Approve(_) => {
            hints.one("confirm", "send").one("close", "don't send");
        }
        Popup::Settings => {
            hints
                .one("keys", "keys")
                .one("theme", "theme")
                .one("migrate", "move secrets")
                .one("close", "close");
        }
        Popup::Welcome { step, .. } => {
            let next = if *step == Step::Keys { "next" } else { "done" };

            hints
                .pair("up", "down", "choose")
                .one("confirm", next)
                .one("close", "keep these");
        }
        Popup::Tabs { .. } => {
            hints
                .range("request.go-1", "request.go-last", "go to")
                .pair("up", "down", "choose")
                .one("confirm", "open")
                .one("request.new", "new")
                .one("request.close", "close")
                .one("close", "back");
        }
        Popup::Environments { .. } => {
            hints
                .pair("up", "down", "choose")
                .one("confirm", "use")
                .one("env.new", "new")
                .one("env.rename", "rename")
                .one("env.delete", "delete")
                .one("env.variables", "variables")
                .one("close", "close");
        }
        Popup::Help { .. } => {
            hints.pair("up", "down", "scroll").one("close", "close");
        }
        Popup::Palette(_) => {
            hints
                .pair("up", "down", "choose")
                .one("confirm", "run")
                .one("close", "close");
        }
        Popup::Variables(editor) if editor.editing() => {
            if let Some((column, _)) = &editor.table().editing {
                super::cell_hints(&mut hints, *column);
            }
        }
        Popup::Variables(_) => {
            hints
                .pair("tab.previous", "tab.next", "tab")
                .one("row.add", "add")
                .one("edit", "edit")
                .one("row.toggle", "on/off")
                .one("make-secret", "secret")
                .one("row.delete", "delete");
        }
    }

    hints.finish()
}
