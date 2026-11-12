mod request;
mod response;

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use crate::app::{AuthKind, BodyKind, Focus, RequestTab, State};

const WIDE: u16 = 110;

pub(crate) fn view(state: &State, frame: &mut Frame) {
    let [top, panes, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    url_bar(state, frame, top);

    let direction = if frame.area().width >= WIDE {
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
    } else {
        Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)])
    };
    let [left, right] = direction.areas(panes);

    request::draw(state, frame, left);
    response::draw(state, frame, right);
    draw_footer(state, frame, footer);

    if let Some(prompt) = &state.save_prompt {
        save_prompt(prompt, frame);
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

    let title = if state.in_flight.is_some() {
        Line::from(" URL · sending… ")
    } else {
        Line::from(" URL ")
    };
    let block = pane(title, state.focus == Focus::Url);
    let inner = block.inner(url_area);
    frame.render_widget(block, url_area);
    frame.render_widget(&state.url, inner);
}

fn save_prompt(prompt: &ratatui_textarea::TextArea<'static>, frame: &mut Frame) {
    let [area] = Layout::vertical([Constraint::Length(3)])
        .flex(Flex::Center)
        .areas(frame.area());
    let [area] = Layout::horizontal([Constraint::Max(60)])
        .flex(Flex::Center)
        .areas(area);
    let block = pane(Line::from(" Save body to file "), true);
    let inner = block.inner(area);

    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(prompt, inner);
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
    if state.save_prompt.is_some() {
        return vec![("Enter", "save"), ("Esc", "close")];
    }

    let mut hints = Vec::new();

    match state.focus {
        Focus::Url => hints.extend([("Enter", "send"), ("Tab", "next")]),
        Focus::Method => hints.extend([("←→", "change method"), ("Tab", "next")]),
        Focus::Request if state.request_typing() => {
            let multi_line =
                state.request_tab == RequestTab::Body && state.body.kind != BodyKind::Form;

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

            if matches!(state.request_tab, RequestTab::Body | RequestTab::Auth) {
                hints.push(("t", "type"));
            }
        }
        Focus::Response => hints.extend([("←→", "tab"), ("↑↓", "scroll"), ("s", "save")]),
    }

    if state.in_flight.is_some() {
        hints.push(("Esc", "cancel"));
    } else if state.focus != Focus::Url
        && !(state.focus == Focus::Request && state.request_typing())
    {
        hints.push(("Ctrl+R", "send"));
    }

    hints.push(("Ctrl+C", "quit"));

    hints
}

#[cfg(test)]
mod tests;
