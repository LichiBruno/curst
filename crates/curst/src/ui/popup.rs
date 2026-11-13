use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use super::request::table;
use super::{pane, tab_titles};
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

pub(super) fn hints(popup: &Popup) -> Vec<(&'static str, &'static str)> {
    match popup {
        Popup::Prompt { .. } => vec![("Enter", "ok"), ("Esc", "cancel")],
        Popup::Confirm { .. } => vec![("y", "yes"), ("n", "no")],
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
            ("d", "delete"),
            ("Ctrl+S", "save"),
            ("Esc", "close"),
        ],
    }
}
