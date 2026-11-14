use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use curst_core::model::{Auth, Request};
use curst_core::secrets::detect;

use super::{field, pane, tab_regions, tab_titles};
use crate::app::{
    AuthKind, BodyField, BodyKind, Column, Field, Focus, Layer, RequestTab, RowCell, State, Table,
    Target, display,
};
use crate::theme::Theme;

pub(super) fn draw(state: &State, frame: &mut Frame, area: Rect) {
    let theme = &state.theme;
    let focused = state.focus == Focus::Request;
    let mut title = vec![Span::raw(" Request")];

    title.extend(tab_titles(
        theme,
        &RequestTab::ALL,
        state.request_tab,
        RequestTab::label,
    ));
    state.region(area, Target::Pane(Focus::Request));
    tab_regions(state, area, &title, 2, |i| Target::Tab(Focus::Request, i));

    let block = pane(theme, Line::from(title), focused);
    let mut inner = block.inner(area);

    frame.render_widget(block, area);

    // only when the pane keeps room for the field the warning is about
    if let Some(warning) = credential_warning(state).filter(|_| inner.height > 2) {
        let [content, line] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);

        frame.render_widget(Paragraph::new(Span::styled(warning, theme.warning)), line);
        inner = content;
    }

    let add = state.key(Layer::Request, "row.add");

    match state.request_tab {
        RequestTab::Params => table(
            state,
            &state.params,
            ["Parameter", "Value"],
            focused,
            &format!("no query parameters, press {add} to add one"),
            frame,
            inner,
        ),
        RequestTab::Headers => table(
            state,
            &state.headers,
            ["Header", "Value"],
            focused,
            &format!("no headers, press {add} to add one"),
            frame,
            inner,
        ),
        RequestTab::Body => body(state, focused, frame, inner),
        RequestTab::Auth => auth(state, focused, frame, inner),
    }
}

/// a warning when the selected row, or the auth, holds what looks like a
/// credential typed in plain text
pub(super) fn credential_warning(state: &State) -> Option<String> {
    let row = |table: &Table, header: bool| {
        table
            .rows
            .get(table.selected)
            .and_then(|r| detect::check_pair(&r.name, &r.value, header))
    };
    let reason = match state.request_tab {
        RequestTab::Params => row(&state.params, false),
        RequestTab::Headers => row(&state.headers, true),
        RequestTab::Body if state.body.kind == BodyKind::Form => row(&state.body.form, false),
        RequestTab::Body => None,
        RequestTab::Auth => {
            let request = Request {
                auth: state.auth.auth(),
                ..Request::default()
            };

            detect::detect(&request).first().map(|f| f.reason)
        }
    }?;

    Some(format!("⚠ {}, saved as plain text", reason.describe()))
}

fn kind_line(theme: &Theme, label: &str, value: &str, key: &str) -> Paragraph<'static> {
    Paragraph::new(Line::from(vec![
        Span::styled(format!("{label}: "), theme.muted),
        Span::styled(
            value.to_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  ({key} to change)"), theme.muted),
    ]))
}

fn body(state: &State, focused: bool, frame: &mut Frame, area: Rect) {
    let theme = &state.theme;
    let [kind_area, content] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);

    let add = state.key(Layer::Request, "row.add");
    let kind = state.key(Layer::Request, "kind");
    let text = field(
        theme,
        &state.body.text,
        Some(format!(
            "press {} to write the body",
            state.key(Layer::Request, "edit")
        )),
    );

    frame.render_widget(
        kind_line(theme, "Body type", state.body.kind.label(), &kind),
        kind_area,
    );

    match state.body.kind {
        BodyKind::None => {}
        BodyKind::Form => table(
            state,
            &state.body.form,
            ["Field", "Value"],
            focused,
            &format!("no form fields, press {add} to add one"),
            frame,
            content,
        ),
        BodyKind::Json => {
            state.region(content, Target::Text(Field::Body));
            frame.render_widget(&*text, content);
        }
        BodyKind::Text => {
            let [type_area, text_area] =
                Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(content);
            let [label_area, field_area] =
                Layout::horizontal([Constraint::Length(14), Constraint::Fill(1)]).areas(type_area);
            let selected = focused && state.body.typing == Some(BodyField::ContentType);
            let style = if selected {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                theme.muted
            };

            frame.render_widget(
                Paragraph::new(Span::styled("Content-Type:", style)),
                label_area,
            );
            frame.render_widget(&*field(theme, &state.body.content_type, None), field_area);
            state.region(text_area, Target::Text(Field::Body));
            frame.render_widget(&*text, text_area);
        }
    }
}

fn auth(state: &State, focused: bool, frame: &mut Frame, area: Rect) {
    let theme = &state.theme;
    let [kind_area, content] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);

    frame.render_widget(
        kind_line(
            theme,
            "Auth type",
            state.auth.kind.label(),
            &state.key(Layer::Request, "kind"),
        ),
        kind_area,
    );

    if state.auth.kind == AuthKind::Inherit {
        let text = match state.inherited_auth() {
            Some((auth, from)) => format!("{} from {}", auth_label(&auth), display(&from)),
            None => "none: the request is in no collection".into(),
        };

        frame.render_widget(
            Paragraph::new(Span::styled(text, theme.muted)),
            row_area(content, 0).unwrap_or(content),
        );

        return;
    }

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
            theme.muted
        };
        let marker = if selected { "› " } else { "  " };

        frame.render_widget(
            Paragraph::new(Span::styled(format!("{marker}{label}"), style)),
            label_area,
        );
        frame.render_widget(&*super::field(theme, field, None), field_area);
    }
}

/// what an auth is, in a few words
pub(super) fn auth_label(auth: &Auth) -> &'static str {
    match auth {
        Auth::Inherit => "inherit",
        Auth::None => "none",
        Auth::Basic { .. } => "basic",
        Auth::Bearer { .. } => "bearer token",
        Auth::ApiKey { .. } => "API key",
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

/// a table of names and values under a line naming its two columns; an
/// empty cell shows what belongs in it
pub(super) fn table(
    state: &State,
    table: &Table,
    columns: [&str; 2],
    focused: bool,
    empty: &str,
    frame: &mut Frame,
    area: Rect,
) {
    let theme = &state.theme;

    if table.rows.is_empty() && table.editing.is_none() {
        frame.render_widget(
            Paragraph::new(Span::styled(empty.to_owned(), theme.muted)),
            area,
        );

        return;
    }

    let cells = |line: Rect| -> [Rect; 3] {
        Layout::horizontal([
            Constraint::Length(4),
            Constraint::Percentage(35),
            Constraint::Fill(1),
        ])
        .spacing(1)
        .areas(line)
    };
    let heading = theme.muted.add_modifier(Modifier::BOLD);

    // the line naming the columns, when a row still fits under it
    let top = usize::from(area.height >= 2);

    if let Some(line) = row_area(area, 0).filter(|_| top == 1) {
        let [_, name_area, value_area] = cells(line);

        for (cell, label) in [(name_area, columns[0]), (value_area, columns[1])] {
            frame.render_widget(
                Paragraph::new(Span::styled(label.to_owned(), heading)),
                cell,
            );
        }
    }

    let height = usize::from(area.height).saturating_sub(top);
    let first = table.selected.saturating_sub(height.saturating_sub(1));

    for (index, row) in table.rows.iter().enumerate().skip(first).take(height) {
        let Some(line) = row_area(area, index - first + top) else {
            break;
        };

        let [check, name_area, value_area] = cells(line);

        state.region(check, Target::Row(index, RowCell::Check));
        state.region(name_area, Target::Row(index, RowCell::Name));
        state.region(value_area, Target::Row(index, RowCell::Value));

        let selected = focused && index == table.selected;
        let mut style = Style::default();

        if !row.enabled {
            style = style.patch(theme.muted).add_modifier(Modifier::CROSSED_OUT);
        }

        if selected {
            style = style.patch(theme.selected);
        }

        let mark = if row.enabled { "[x]" } else { "[ ]" };

        frame.render_widget(Paragraph::new(Span::raw(mark)), check);

        let editing = table
            .editing
            .as_ref()
            .filter(|_| index == table.selected)
            .map(|(column, area)| (*column, area));

        let value = if row.secret {
            "•••••• secret".to_owned()
        } else {
            row.value.clone()
        };

        for (column, cell, text, label) in [
            (Column::Name, name_area, &row.name, columns[0]),
            (Column::Value, value_area, &value, columns[1]),
        ] {
            let empty = Span::styled(label.to_lowercase(), style.patch(theme.muted));

            match editing {
                Some((edited, area)) if edited == column => {
                    // a secret's value keeps its own explanation
                    let placeholder =
                        (column == Column::Name || !row.secret).then(|| label.to_lowercase());

                    frame.render_widget(&*field(theme, area, placeholder), cell);
                }
                _ if text.is_empty() => frame.render_widget(Paragraph::new(empty), cell),
                _ => frame.render_widget(Paragraph::new(Span::styled(text.clone(), style)), cell),
            }
        }
    }
}
