use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use super::request::table;
use super::{pane, tab_titles};
use curst_core::model::LocalId;
use curst_core::secrets::{Approval, SecretKey};
use curst_core::store::Backend;

use ratatui_textarea::TextArea;

use crate::app::{Popup, State, VarTab, display};

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
    match popup {
        Popup::Prompt { purpose, input } => {
            let area = centered(frame, 60, 3);
            let block = pane(Line::from(format!(" {} ", purpose.title())), true);
            let inner = block.inner(area);

            frame.render_widget(Clear, area);
            frame.render_widget(block, area);
            frame.render_widget(&**input, inner);
        }
        Popup::Confirm { message, .. } => {
            let area = centered(frame, 60, 5);
            let block = pane(Line::from(" Are you sure? "), true);
            let text = vec![
                Line::from(display(message)),
                Line::from(""),
                Line::from(vec![
                    Span::styled(" y ", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw("yes   "),
                    Span::styled(" n ", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw("no"),
                ]),
            ];

            frame.render_widget(Clear, area);
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
            let block = pane(Line::from(title), true);
            let names = std::iter::once((None, "No environment".to_owned()))
                .chain(environments.iter().map(|e| (Some(&e.id), display(&e.name))));
            let mut lines: Vec<Line> = names
                .enumerate()
                .map(|(index, (id, name))| {
                    let active = id == workspace.active_environment.as_ref();
                    let mark = if active { "✓ " } else { "  " };
                    let style = if index == *selected {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default()
                    };

                    Line::from(Span::styled(format!("{mark}{name}"), style))
                })
                .collect();

            if environments.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  press n to add one",
                    Style::default().fg(Color::DarkGray),
                )));
            }

            frame.render_widget(Clear, area);
            frame.render_widget(Paragraph::new(lines).block(block), area);
        }
        Popup::Trust { collection } => trust(state, collection, frame),
        Popup::ChooseBackend => choose_backend(frame),
        Popup::Passphrase {
            new,
            first,
            input,
            error,
            ..
        } => passphrase(*new, first.is_some(), input, error.as_deref(), frame),
        Popup::Settings => settings(state, frame),
        Popup::Approve(approval) => approve(approval, frame),
        Popup::Variables(editor) => {
            let area = centered(frame, 100, frame.area().height.saturating_sub(4).max(8));
            let label = |tab: VarTab| match (tab, &editor.environment) {
                (VarTab::Environment, Some(_)) => "Environment",
                (tab, _) => tab.label(),
            };
            let mut title = vec![Span::raw(" Variables")];

            title.extend(tab_titles(&editor.tabs(), editor.tab, label));

            if let (VarTab::Environment, Some(environment)) = (editor.tab, &editor.environment) {
                title.push(Span::raw(format!("· {} ", display(&environment.name))));
            }

            let block = pane(Line::from(title), true);
            let inner = block.inner(area);
            let empty = match editor.tab {
                VarTab::Environment => "no variables in this environment, press a to add one",
                VarTab::Collection => "no collection variables, press a to add one",
                VarTab::Globals => "no global variables, press a to add one",
            };

            frame.render_widget(Clear, area);
            frame.render_widget(block, area);
            table(editor.table(), true, empty, frame, inner);
        }
    }
}

fn settings(state: &State, frame: &mut Frame) {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Color::DarkGray);
    let (now, action) = match state.backend {
        None => ("not chosen yet", "choose where to keep them"),
        Some(Backend::Keychain) => ("the system keychain", "move them to an encrypted file"),
        Some(Backend::EncryptedFile) => (
            "an encrypted file, secrets.age",
            "move them to the keychain",
        ),
    };
    let lines = vec![
        Line::from(vec![
            Span::raw("Secrets are kept in "),
            Span::styled(now, bold),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(" m ", bold), Span::raw(action)]),
        Line::from(""),
        Line::from(Span::styled(
            "Moving copies every secret, checks each copy, then deletes the old ones.",
            dim,
        )),
    ];
    let area = centered(frame, 76, 7);

    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(pane(Line::from(" Settings "), true)),
        area,
    );
}

fn choose_backend(frame: &mut Frame) {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let lines = vec![
        Line::from("This request uses a secret. Where should curst keep secrets?"),
        Line::from(""),
        Line::from(vec![
            Span::styled(" k ", bold),
            Span::raw("the system keychain: nothing to type, but it stays on"),
        ]),
        Line::from("    this machine, and some Linux setups have none"),
        Line::from(vec![
            Span::styled(" f ", bold),
            Span::raw("an encrypted file: works everywhere, asks for a"),
        ]),
        Line::from("    passphrase once per session"),
        Line::from(""),
        Line::from(Span::styled(
            "You can switch later in Settings.",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let area = centered(frame, 68, 10);

    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(pane(Line::from(" Keep secrets where? "), true)),
        area,
    );
}

fn passphrase(
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
    let area = centered(frame, 60, 4);
    let block = pane(Line::from(title), true);
    let inner = block.inner(area);
    let [field, message] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);

    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(input, field);

    if let Some(error) = error {
        frame.render_widget(
            Paragraph::new(Span::styled(
                error.to_owned(),
                Style::default().fg(Color::Yellow),
            )),
            message,
        );
    }
}

fn approve(approval: &Approval, frame: &mut Frame) {
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
            Style::default().fg(Color::Yellow),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(" y ", bold),
        Span::raw("send them there, now and later   "),
        Span::styled(" n ", bold),
        Span::raw("don't send"),
    ]));

    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let area = centered(frame, 70, height);

    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(pane(Line::from(" Send secrets? "), true)),
        area,
    );
}

fn trust(state: &State, collection: &LocalId, frame: &mut Frame) {
    let Some(workspace) = state.workspace(collection) else {
        return;
    };

    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Color::DarkGray);
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
            Span::styled(" y ", bold),
            Span::raw("trust it as a new collection   "),
            Span::styled(" n ", bold),
            Span::raw("not now, only look"),
        ]),
    ];

    if !state.orphans.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(
            "or, if it was moved from one of these folders, keep its old secrets:",
        ));

        for (index, (_, old)) in state.orphans.iter().take(9).enumerate() {
            lines.push(Line::from(vec![
                Span::styled(format!(" {} ", index + 1), bold),
                Span::raw(display(&old.display().to_string())),
            ]));
        }
    }

    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let area = centered(frame, 76, height);
    let block = pane(Line::from(" Trust this folder? "), true);

    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

pub(super) fn hints(popup: &Popup) -> Vec<(&'static str, &'static str)> {
    match popup {
        Popup::Prompt { .. } | Popup::Passphrase { .. } => vec![("Enter", "ok"), ("Esc", "cancel")],
        Popup::Confirm { .. } => vec![("y", "yes"), ("n", "no")],
        Popup::Trust { .. } => vec![("y", "trust"), ("n", "not now"), ("1-9", "moved from")],
        Popup::ChooseBackend => vec![("k", "keychain"), ("f", "encrypted file"), ("Esc", "later")],
        Popup::Approve(_) => vec![("y", "send"), ("n", "don't send")],
        Popup::Settings => vec![("m", "move secrets"), ("Esc", "close")],
        Popup::Environments { .. } => vec![
            ("↑↓", "choose"),
            ("Enter", "use"),
            ("n", "new"),
            ("r", "rename"),
            ("d", "delete"),
            ("v", "variables"),
            ("Esc", "close"),
        ],
        Popup::Variables(editor) if editor.editing() => {
            vec![("Enter", "done"), ("Tab", "next field")]
        }
        Popup::Variables(_) => vec![
            ("←→", "tab"),
            ("a", "add"),
            ("Enter", "edit"),
            ("Space", "on/off"),
            ("s", "secret"),
            ("d", "delete"),
        ],
    }
}
