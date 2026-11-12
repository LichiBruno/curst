use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{pane, tab_titles};
use crate::app::{AuthKind, BodyKind, Column, Focus, RequestTab, State, Table};

pub(super) fn draw(state: &State, frame: &mut Frame, area: Rect) {
    let focused = state.focus == Focus::Request;
    let mut title = vec![Span::raw(" Request")];
    title.extend(tab_titles(
        &RequestTab::ALL,
        state.request_tab,
        RequestTab::label,
    ));
    let block = pane(Line::from(title), focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    match state.request_tab {
        RequestTab::Params => table(
            &state.params,
            focused,
            "no query parameters, press a to add one",
            frame,
            inner,
        ),
        RequestTab::Headers => table(
            &state.headers,
            focused,
            "no headers, press a to add one",
            frame,
            inner,
        ),
        RequestTab::Body => body(state, focused, frame, inner),
        RequestTab::Auth => auth(state, focused, frame, inner),
    }
}

fn kind_line(label: &str, value: &str) -> Paragraph<'static> {
    Paragraph::new(Line::from(vec![
        Span::styled(format!("{label}: "), Style::default().fg(Color::DarkGray)),
        Span::styled(
            value.to_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled("  (t to change)", Style::default().fg(Color::DarkGray)),
    ]))
}

fn body(state: &State, focused: bool, frame: &mut Frame, area: Rect) {
    let [kind_area, content] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    frame.render_widget(kind_line("Body type", state.body.kind.label()), kind_area);

    match state.body.kind {
        BodyKind::None => {}
        BodyKind::Form => table(
            &state.body.form,
            focused,
            "no form fields, press a to add one",
            frame,
            content,
        ),
        BodyKind::Json | BodyKind::Text => frame.render_widget(&state.body.text, content),
    }
}

fn auth(state: &State, focused: bool, frame: &mut Frame, area: Rect) {
    let [kind_area, content] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    frame.render_widget(kind_line("Auth type", state.auth.kind.label()), kind_area);

    if state.auth.kind == AuthKind::None {
        return;
    }

    let first = state
        .auth
        .selected
        .saturating_sub(usize::from(content.height).saturating_sub(1));

    for (index, (label, field)) in state.auth.fields().into_iter().enumerate().skip(first) {
        let Some(row) = row_area(content, index - first) else {
            break;
        };

        let [label_area, field_area] =
            Layout::horizontal([Constraint::Length(13), Constraint::Fill(1)]).areas(row);
        let selected = focused && index == state.auth.selected;
        let style = if selected {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let marker = if selected { "› " } else { "  " };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("{marker}{label}"), style)),
            label_area,
        );
        frame.render_widget(field, field_area);
    }
}

fn row_area(area: Rect, index: usize) -> Option<Rect> {
    let offset = u16::try_from(index).ok()?;
    (offset < area.height).then(|| Rect {
        y: area.y + offset,
        height: 1,
        ..area
    })
}

fn table(table: &Table, focused: bool, empty: &str, frame: &mut Frame, area: Rect) {
    if table.rows.is_empty() && table.editing.is_none() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                empty.to_owned(),
                Style::default().fg(Color::DarkGray),
            )),
            area,
        );
        return;
    }

    let height = usize::from(area.height);
    let first = table.selected.saturating_sub(height.saturating_sub(1));

    for (index, row) in table.rows.iter().enumerate().skip(first).take(height) {
        let Some(line) = row_area(area, index - first) else {
            break;
        };

        let [check, name_area, value_area] = Layout::horizontal([
            Constraint::Length(4),
            Constraint::Percentage(35),
            Constraint::Fill(1),
        ])
        .spacing(1)
        .areas(line);
        let selected = focused && index == table.selected;
        let mut style = Style::default();

        if !row.enabled {
            style = style
                .fg(Color::DarkGray)
                .add_modifier(Modifier::CROSSED_OUT);
        }

        if selected {
            style = style.add_modifier(Modifier::REVERSED);
        }

        let mark = if row.enabled { "[x]" } else { "[ ]" };
        frame.render_widget(Paragraph::new(Span::raw(mark)), check);
        let editing = table
            .editing
            .as_ref()
            .filter(|_| index == table.selected)
            .map(|(column, area)| (*column, area));

        for (column, cell, text) in [
            (Column::Name, name_area, &row.name),
            (Column::Value, value_area, &row.value),
        ] {
            match editing {
                Some((edited, area)) if edited == column => frame.render_widget(area, cell),
                _ => frame.render_widget(Paragraph::new(Span::styled(text.clone(), style)), cell),
            }
        }
    }
}
