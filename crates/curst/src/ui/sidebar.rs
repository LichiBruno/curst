use curst_core::model::Method;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::pane;
use crate::app::{Focus, Layer, RowKind, State, Target, display, rows};

fn method_tag(method: Method) -> &'static str {
    match method {
        Method::Get => "GET ",
        Method::Post => "POST",
        Method::Put => "PUT ",
        Method::Patch => "PTCH",
        Method::Delete => "DEL ",
        Method::Head => "HEAD",
        Method::Options => "OPT ",
    }
}

pub(super) fn draw(state: &State, frame: &mut Frame, area: Rect) {
    let theme = &state.theme;
    let focused = state.focus == Focus::Sidebar;
    let mut block = pane(theme, Line::from(" Collections "), focused);

    if let Some(name) = state.current_workspace().and_then(|w| w.environment_name()) {
        block = block.title(
            Line::from(Span::styled(format!(" {} ", display(name)), theme.accent)).right_aligned(),
        );
    }

    let inner = block.inner(area);

    state.region(area, Target::Pane(Focus::Sidebar));
    frame.render_widget(block, area);

    let rows = rows(&state.collections, &state.sidebar.expanded);
    let dim = theme.muted;

    if rows.is_empty() {
        let text = vec![
            Line::from(Span::styled("no collections yet", dim)),
            Line::from(Span::styled(
                format!(
                    "press {} to add one",
                    state.key(Layer::Sidebar, "new-collection")
                ),
                dim,
            )),
        ];

        frame.render_widget(Paragraph::new(text), inner);

        return;
    }

    let height = usize::from(inner.height);
    let selected = state
        .sidebar
        .selected
        .as_ref()
        .and_then(|k| rows.iter().position(|r| r.key == *k));
    let first = selected.map_or(0, |s| s.saturating_sub(height.saturating_sub(1)));
    let open = state.open.as_ref().map(|o| &o.key);
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(first)
        .take(height)
        .map(|(index, row)| {
            let line = Rect {
                y: inner.y + u16::try_from(index - first).unwrap_or(0),
                height: 1,
                ..inner
            };

            state.region(line, Target::Node(row.key.clone()));

            let mut spans = vec![Span::raw("  ".repeat(row.depth))];

            match row.kind {
                RowKind::Collection | RowKind::Folder => {
                    spans.push(Span::styled(if row.expanded { "▾ " } else { "▸ " }, dim));
                }
                RowKind::Request(method) => spans.push(Span::styled(
                    format!("{} ", method_tag(method)),
                    theme.method(method),
                )),
            }

            let mut style = Style::default();

            if row.kind == RowKind::Collection {
                style = style.add_modifier(Modifier::BOLD);
            }

            if Some(&row.key) == open {
                style = style.patch(theme.accent);
            }

            if state.sidebar.cut.as_ref() == Some(&row.key) {
                style = style.add_modifier(Modifier::DIM | Modifier::ITALIC);
            }

            if Some(index) == selected {
                style = if focused {
                    style.patch(theme.selected)
                } else {
                    style.add_modifier(Modifier::UNDERLINED)
                };
            }

            spans.push(Span::styled(display(&row.name), style));

            if row.kind == RowKind::Collection && !row.trusted {
                spans.push(Span::styled(" untrusted", theme.warning));
            }

            Line::from(spans)
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}
