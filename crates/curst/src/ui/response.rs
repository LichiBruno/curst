use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{pane, tab_titles};
use crate::app::{Focus, Outcome, ResponseTab, State};

pub(super) fn draw(state: &State, frame: &mut Frame, area: Rect) {
    let mut title = vec![Span::raw(" Response")];
    title.extend(tab_titles(
        &ResponseTab::ALL,
        state.response_tab,
        ResponseTab::label,
    ));
    let mut block = pane(Line::from(title), state.focus == Focus::Response);

    if let Some(Outcome::Response(view)) = &state.outcome {
        let color = match view.response.status {
            200..=299 => Color::Green,
            300..=399 => Color::Cyan,
            400..=499 => Color::Yellow,
            _ => Color::Red,
        };
        block = block.title(
            Line::from(Span::styled(
                format!(" {} ", view.summary),
                Style::default().fg(color),
            ))
            .right_aligned(),
        );
    }

    let inner = block.inner(area);
    frame.render_widget(block, area);
    state.response_height.set(usize::from(inner.height));

    let dim = Style::default().fg(Color::DarkGray);
    let paragraph = match &state.outcome {
        None if state.in_flight.is_some() => Paragraph::new(Span::styled("sending…", dim)),
        None => Paragraph::new(Span::styled(
            "no response yet, type a URL and press Enter",
            dim,
        )),
        Some(Outcome::Error(message)) => Paragraph::new(vec![
            Line::from(Span::styled(
                "the request failed",
                Style::default().fg(Color::Red),
            )),
            Line::from(message.clone()),
        ]),
        Some(Outcome::Response(view)) => {
            let lines = view.lines(state.response_tab);
            let height = usize::from(inner.height);
            let first = state.scroll.min(lines.len().saturating_sub(height));
            Paragraph::new(
                lines
                    .iter()
                    .skip(first)
                    .take(height)
                    .map(|line| Line::from(line.as_str()))
                    .collect::<Vec<_>>(),
            )
        }
    };
    frame.render_widget(paragraph, inner);
}
