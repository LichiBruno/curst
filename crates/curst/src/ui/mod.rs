mod popup;
mod request;
mod response;
mod sidebar;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};

use crate::app::{AuthKind, BodyField, BodyKind, Focus, Popup, RequestTab, State, display};

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
        Layout::vertical([Constraint::Fill(1), Constraint::Length(2)]).areas(frame.area());
    let [side, main] = Layout::horizontal([
        Constraint::Length(sidebar_width(body.width)),
        Constraint::Fill(1),
    ])
    .areas(body);
    let [bar, top, panes] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Fill(1),
    ])
    .areas(main);

    tab_bar(state, frame, bar);

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

/// one label per open request, the active one highlighted; when they do
/// not all fit, those far from the active one are left out
fn tab_bar(state: &State, frame: &mut Frame, area: Rect) {
    let labels = state.tab_labels();
    let texts: Vec<String> = labels
        .iter()
        .map(|label| {
            let mut name = display(&label.name);

            if name.chars().count() > 24 {
                name = name.chars().take(23).collect::<String>() + "…";
            }

            let dirty = if label.dirty { " ●" } else { "" };

            format!(" {} {name}{dirty} ", label.method.as_str())
        })
        .collect();
    let width = |text: &String| text.chars().count() + 1;
    let room = usize::from(area.width).saturating_sub(4);
    let active = labels.iter().position(|l| l.active).unwrap_or(0);
    let (mut first, mut last) = (active, active);
    let mut used = texts.get(active).map_or(0, width);

    // widen around the active tab, one side then the other, while it fits
    loop {
        let right = texts.get(last + 1).map(width).filter(|w| used + w <= room);

        if let Some(w) = right {
            last += 1;
            used += w;
        }

        let left = first
            .checked_sub(1)
            .and_then(|i| texts.get(i))
            .map(width)
            .filter(|w| used + w <= room);

        if let Some(w) = left {
            first -= 1;
            used += w;
        }

        if right.is_none() && left.is_none() {
            break;
        }
    }

    let dim = Style::default().fg(Color::DarkGray);
    let mut spans = vec![Span::styled(if first > 0 { "‹" } else { " " }, dim)];

    for (index, text) in texts.iter().enumerate().take(last + 1).skip(first) {
        let style = if labels[index].active {
            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            dim
        };

        spans.push(Span::styled(text.clone(), style));
        spans.push(Span::raw(" "));
    }

    if last + 1 < texts.len() {
        spans.push(Span::styled("›", dim));
    }

    let hint = " Ctrl+G tabs  Ctrl+T new  Ctrl+W close ";
    let shown: usize = spans.iter().map(|s| s.content.chars().count()).sum();

    frame.render_widget(Paragraph::new(Line::from(spans)), area);

    if shown + hint.chars().count() <= usize::from(area.width) {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(hint, dim)).right_aligned()),
            area,
        );
    }
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

/// two lines: a message and the keys for the focused pane, then the keys
/// that work everywhere, always in the same place
fn draw_footer(state: &State, frame: &mut Frame, area: Rect) {
    let keys = |spans: &mut Vec<Span<'static>>, hints: Vec<(&'static str, &'static str)>| {
        for (key, label) in hints {
            spans.push(Span::styled(format!(" {key} "), Style::default().bold()));
            spans.push(Span::styled(
                format!("{label} "),
                Style::default().fg(Color::DarkGray),
            ));
        }
    };
    let mut here: Vec<Span<'static>> = Vec::new();

    if let Some(notice) = &state.notice {
        here.push(Span::styled(
            format!(" {notice} "),
            Style::default().fg(Color::Yellow),
        ));
        here.push(Span::raw(" "));
    }

    keys(&mut here, hints(state));

    let mut everywhere = Vec::new();

    keys(&mut everywhere, global_hints(state));

    let [top, bottom] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);

    frame.render_widget(Paragraph::new(Line::from(here)), top);
    frame.render_widget(Paragraph::new(Line::from(everywhere)), bottom);
}

/// the keys that work whatever has the focus; with a pop-up open, the
/// keys that finish it, and quitting
pub(crate) fn global_hints(state: &State) -> Vec<(&'static str, &'static str)> {
    match &state.popup {
        Some(Popup::Variables(editor)) if !editor.editing() => {
            return vec![("Ctrl+S", "save"), ("Esc", "close"), ("Ctrl+C", "quit")];
        }
        Some(_) => return vec![("Ctrl+C", "quit")],
        None => {}
    }

    vec![
        ("Ctrl+R", "send"),
        ("Ctrl+S", "save"),
        ("Ctrl+E", "environments"),
        ("Ctrl+O", "settings"),
        ("Ctrl+C", "quit"),
    ]
}

/// the keys that work where the focus is; the ones that work everywhere
/// are in [`global_hints`]
pub(crate) fn hints(state: &State) -> Vec<(&'static str, &'static str)> {
    if let Some(popup) = &state.popup {
        return popup::hints(popup);
    }

    let mut hints = Vec::new();

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

            if request::credential_warning(state).is_some() {
                hints.push(("m", "make it a secret"));
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
    }

    hints
}

#[cfg(test)]
mod tests;
