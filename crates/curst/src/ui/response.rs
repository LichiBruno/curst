use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{pane, tab_regions, tab_titles};
use crate::app::{Focus, Layer, Outcome, ResponseTab, State, Target, skip_columns};

pub(super) fn draw(state: &State, frame: &mut Frame, area: Rect) {
    let theme = &state.theme;
    let mut title = vec![Span::raw(" Response")];

    title.extend(tab_titles(
        theme,
        &ResponseTab::ALL,
        state.response_tab,
        ResponseTab::label,
    ));

    state.region(area, Target::Pane(Focus::Response));
    tab_regions(state, area, &title, 2, |i| Target::Tab(Focus::Response, i));

    let mut block = pane(theme, Line::from(title), state.focus == Focus::Response);

    if let Some(Outcome::Response(view)) = &state.outcome {
        let style = match view.response.status {
            200..=299 => theme.success,
            300..=399 => theme.info,
            400..=499 => theme.warning,
            _ => theme.error,
        };

        // on the bottom border, so a narrow pane keeps its tab names
        block = block.title_bottom(
            Line::from(Span::styled(format!(" {} ", view.summary), style)).right_aligned(),
        );
    }

    let inner = block.inner(area);

    frame.render_widget(block, area);
    state.response_height.set(usize::from(inner.height));
    state.response_width.set(usize::from(inner.width));

    let dim = theme.muted;
    let paragraph = match &state.outcome {
        None if state.in_flight.is_some() => Paragraph::new(Span::styled("sending…", dim)),
        None => Paragraph::new(Span::styled(
            format!(
                "no response yet, type a URL and press {}",
                state.key(Layer::Url, "send")
            ),
            dim,
        )),
        Some(Outcome::Error(message)) => Paragraph::new(vec![
            Line::from(Span::styled("the request failed", theme.error)),
            Line::from(message.clone()),
        ]),
        Some(Outcome::Response(view)) => {
            let lines = view.lines(state.response_tab);
            let height = usize::from(inner.height);
            let first = state.scroll.min(lines.len().saturating_sub(height));
            let column = state.column.min(state.last_column());

            Paragraph::new(
                lines
                    .iter()
                    .skip(first)
                    .take(height)
                    .map(|line| Line::from(skip_columns(line, column)))
                    .collect::<Vec<_>>(),
            )
        }
    };

    frame.render_widget(paragraph, inner);
}
