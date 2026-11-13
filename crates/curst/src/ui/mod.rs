mod popup;
mod request;
mod response;
mod sidebar;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};

use crate::app::{AuthKind, BodyField, BodyKind, Focus, RequestTab, State, display};

const WIDE: u16 = 110;

/// the sidebar's width for a terminal this wide, 0 when it is too narrow
fn sidebar_width(width: u16) -> u16 {
    match width {
        100.. => 30,
        60.. => 24,
        _ => 0,
    }
}

pub(crate) fn view(state: &State, frame: &mut Frame) {
    let [body, footer] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
    let [side, main] = Layout::horizontal([
        Constraint::Length(sidebar_width(body.width)),
        Constraint::Fill(1),
    ])
    .areas(body);
    let [top, panes] = Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(main);

    if side.width > 0 {
        sidebar::draw(state, frame, side);
    }

    url_bar(state, frame, top);

    let direction = if main.width >= WIDE {
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
    } else {
        Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)])
    };
    let [left, right] = direction.areas(panes);

    request::draw(state, frame, left);
    response::draw(state, frame, right);
    draw_footer(state, frame, footer);

    if let Some(popup) = &state.popup {
        popup::draw(state, popup, frame);
    }
}

pub(super) fn pane(title: Line<'static>, focused: bool) -> Block<'static> {
    let block = Block::bordered().title(title);

    if focused {
        block
            .border_type(BorderType::Thick)
            .border_style(Style::default().fg(Color::Cyan))
    } else {
        block.border_style(Style::default().fg(Color::DarkGray))
    }
}

pub(super) fn tab_titles<T: Copy + PartialEq>(
    all: &[T],
    selected: T,
    label: impl Fn(T) -> &'static str,
) -> Vec<Span<'static>> {
    let mut spans = vec![Span::raw(" ")];

    for tab in all {
        let text = format!(" {} ", label(*tab));

        spans.push(if *tab == selected {
            Span::styled(text, Style::default().add_modifier(Modifier::REVERSED))
        } else {
            Span::raw(text)
        });
    }

    spans.push(Span::raw(" "));

    spans
}

fn url_bar(state: &State, frame: &mut Frame, area: Rect) {
    let [method_area, url_area] =
        Layout::horizontal([Constraint::Length(11), Constraint::Fill(1)]).areas(area);
    let method = Paragraph::new(Line::from(state.method.as_str()).bold().centered())
        .block(pane(Line::from(" Method "), state.focus == Focus::Method));

    frame.render_widget(method, method_area);

    let mut title = match &state.open {
        Some(open) => format!(" {} ", display(&open.name)),
        None => " URL ".to_owned(),
    };

    if state.is_dirty() {
        title.push_str("● ");
    }

    if state.in_flight.is_some() {
        title.push_str("· sending… ");
    }

    let block = pane(Line::from(title), state.focus == Focus::Url);
    let inner = block.inner(url_area);

    frame.render_widget(block, url_area);
    frame.render_widget(&state.url, inner);
}

fn draw_footer(state: &State, frame: &mut Frame, area: Rect) {
    let mut spans: Vec<Span<'static>> = Vec::new();

    if let Some(notice) = &state.notice {
        spans.push(Span::styled(
            format!(" {notice} "),
            Style::default().fg(Color::Yellow),
        ));
        spans.push(Span::raw(" "));
    }

    for (key, label) in hints(state) {
        spans.push(Span::styled(format!(" {key} "), Style::default().bold()));
        spans.push(Span::styled(
            format!("{label} "),
            Style::default().fg(Color::DarkGray),
        ));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// the keys that work where the focus is, so they can be shown in the footer
pub(crate) fn hints(state: &State) -> Vec<(&'static str, &'static str)> {
    if let Some(popup) = &state.popup {
        return popup::hints(popup);
    }

    let mut hints = Vec::new();

    // the most urgent key comes first, before the footer runs out of room
    if state.is_dirty() {
        hints.push(("Ctrl+S", "save"));
    }

    match state.focus {
        Focus::Sidebar => hints.extend([
            ("Enter", "open"),
            ("n", "request"),
            ("f", "folder"),
            ("c", "collection"),
            ("r", "rename"),
            ("d", "delete"),
            ("x p", "move"),
        ]),
        Focus::Url => hints.extend([("Enter", "send"), ("Tab", "next")]),
        Focus::Method => hints.extend([("←→", "change method"), ("Tab", "next")]),
        Focus::Request if state.request_typing() => {
            let multi_line =
                state.request_tab == RequestTab::Body && state.body.typing == Some(BodyField::Text);

            if multi_line {
                hints.push(("Esc", "done"));
            } else {
                hints.extend([("Enter", "done"), ("Tab", "next field")]);
            }
        }
        Focus::Request => {
            hints.push(("←→", "tab"));

            let table = matches!(state.request_tab, RequestTab::Params | RequestTab::Headers)
                || (state.request_tab == RequestTab::Body && state.body.kind == BodyKind::Form);

            if table {
                hints.extend([
                    ("a", "add"),
                    ("Enter", "edit"),
                    ("Space", "on/off"),
                    ("d", "delete"),
                ]);
            } else if state.request_tab == RequestTab::Auth && state.auth.kind != AuthKind::None
                || state.request_tab == RequestTab::Body && state.body.kind != BodyKind::None
            {
                hints.push(("Enter", "edit"));
            }

            if state.request_tab == RequestTab::Body && state.body.kind == BodyKind::Text {
                hints.push(("c", "content type"));
            }

            if matches!(state.request_tab, RequestTab::Body | RequestTab::Auth) {
                hints.push(("t", "type"));
            }
        }
        Focus::Response => {
            hints.extend([("←→", "tab"), ("↑↓", "scroll")]);

            if state.last_column() > 0 || state.column > 0 {
                hints.push(("h l", "sideways"));
            }

            hints.push(("s", "save"));
        }
    }

    if state.in_flight.is_some() {
        hints.push(("Esc", "cancel"));
    } else if state.focus != Focus::Url
        && !(state.focus == Focus::Request && state.request_typing())
    {
        hints.push(("Ctrl+R", "send"));
    }

    if !state.collections.is_empty() {
        hints.push(("Ctrl+E", "env"));
    }

    hints.push(("Ctrl+C", "quit"));

    hints
}

#[cfg(test)]
mod tests;
