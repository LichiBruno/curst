mod popup;
mod request;
mod response;
mod sidebar;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};

use std::borrow::Cow;
use std::fmt::Write;

use unicode_width::UnicodeWidthStr;

use ratatui_textarea::TextArea;

use crate::app::{
    AuthKind, BodyKind, Column, Field, Focus, Hint, Hints, Layer, Popup, RequestTab, State, Target,
    display,
};
use crate::theme::Theme;

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
    state.clear_regions();

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

pub(super) fn pane(theme: &Theme, title: Line<'static>, focused: bool) -> Block<'static> {
    let block = Block::bordered().title(title);

    if focused {
        block
            .border_type(BorderType::Thick)
            .border_style(theme.accent)
    } else {
        block.border_style(theme.border)
    }
}

/// records where the tab titles on the top border of `area` are: the
/// spans of `title` from `first` on are the tabs, as [`tab_titles`] makes
/// them, and the last span is only padding
pub(super) fn tab_regions(
    state: &State,
    area: Rect,
    title: &[Span],
    first: usize,
    target: impl Fn(usize) -> Target,
) {
    // a title starts after the corner of the border
    let mut x = usize::from(area.x) + 1 + title.iter().take(first).map(Span::width).sum::<usize>();
    let tabs = title.len().saturating_sub(first + 1);

    for (index, span) in title.iter().skip(first).take(tabs).enumerate() {
        let width = span.width();
        let tab = Rect::new(
            u16::try_from(x).unwrap_or(u16::MAX),
            area.y,
            u16::try_from(width).unwrap_or(0),
            1,
        );

        state.region(tab.intersection(area), target(index));
        x += width;
    }
}

pub(super) fn tab_titles<T: Copy + PartialEq>(
    theme: &Theme,
    all: &[T],
    selected: T,
    label: impl Fn(T) -> &'static str,
) -> Vec<Span<'static>> {
    let mut spans = vec![Span::raw(" ")];

    for tab in all {
        let text = format!(" {} ", label(*tab));

        spans.push(if *tab == selected {
            Span::styled(text, theme.selected)
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
    let theme = &state.theme;
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

    let dim = theme.muted;
    let mut spans = vec![Span::styled(if first > 0 { "‹" } else { " " }, dim)];

    for (index, text) in texts.iter().enumerate().take(last + 1).skip(first) {
        let x: usize = spans.iter().map(Span::width).sum();
        let tab = Rect::new(
            area.x.saturating_add(u16::try_from(x).unwrap_or(u16::MAX)),
            area.y,
            u16::try_from(text.width()).unwrap_or(0),
            1,
        );

        state.region(tab.intersection(area), Target::Request(index));

        let style = if labels[index].active {
            theme.selected.add_modifier(Modifier::BOLD)
        } else {
            dim
        };

        spans.push(Span::styled(text.clone(), style));
        spans.push(Span::raw(" "));
    }

    if last + 1 < texts.len() {
        spans.push(Span::styled("›", dim));
    }

    let hint: String = Hints::everywhere(state)
        .one("tabs", "tabs")
        .one("request.new", "new")
        .one("request.close", "close")
        .finish()
        .iter()
        .fold(String::new(), |mut text, hint| {
            let _ = write!(text, " {} {} ", hint.key, hint.label);

            text
        });
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
    let theme = &state.theme;
    let [method_area, url_area] =
        Layout::horizontal([Constraint::Length(11), Constraint::Fill(1)]).areas(area);
    let method = Paragraph::new(Line::from(state.method.as_str()).bold().centered()).block(pane(
        theme,
        Line::from(" Method "),
        state.focus == Focus::Method,
    ));

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

    let block = pane(theme, Line::from(title), state.focus == Focus::Url);
    let inner = block.inner(url_area);

    state.region(method_area, Target::Method);
    state.region(url_area, Target::Pane(Focus::Url));
    state.region(inner, Target::Text(Field::Url));

    frame.render_widget(block, url_area);
    let placeholder = format!(
        "https://example.com/api, then {} to send",
        state.key(Layer::Url, "send")
    );

    frame.render_widget(&*field(theme, &state.url, Some(placeholder)), inner);
}

/// a text field to draw, its placeholder in the theme's colors and, when
/// given, naming the current keys; only an empty field shows one, so only
/// an empty field is copied
pub(super) fn field<'a>(
    theme: &Theme,
    area: &'a TextArea<'static>,
    placeholder: Option<String>,
) -> Cow<'a, TextArea<'static>> {
    if !area.is_empty() {
        return Cow::Borrowed(area);
    }

    let mut area = area.clone();

    area.set_placeholder_style(theme.muted);

    if let Some(placeholder) = placeholder {
        area.set_placeholder_text(placeholder);
    }

    Cow::Owned(area)
}

/// two lines: a message and the keys for the focused pane, then the keys
/// that work everywhere, always in the same place
fn draw_footer(state: &State, frame: &mut Frame, area: Rect) {
    let theme = &state.theme;
    let [top, bottom] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);

    // each hint is a button for its action, where it has one
    let keys = |spans: &mut Vec<Span<'static>>, line: Rect, hints: Vec<Hint>| {
        let mut x: usize = spans.iter().map(Span::width).sum();

        for hint in hints {
            let key = Span::styled(format!(" {} ", hint.key), theme.key);
            let label = Span::styled(format!("{} ", hint.label), theme.muted);
            let width = key.width() + label.width();

            if let Some(name) = hint.name {
                let at = line.x.saturating_add(u16::try_from(x).unwrap_or(u16::MAX));
                let button = Rect::new(at, line.y, u16::try_from(width).unwrap_or(0), 1);

                state.region(button.intersection(line), Target::Run(name));
            }

            spans.extend([key, label]);
            x += width;
        }
    };
    let mut here: Vec<Span<'static>> = Vec::new();

    if let Some(pending) = state.pending_label() {
        here.push(Span::styled(format!(" {pending} … "), theme.key));
    }

    if let Some(notice) = &state.notice {
        here.push(Span::styled(format!(" {notice} "), theme.warning));
        here.push(Span::raw(" "));
    }

    keys(&mut here, top, hints(state));

    let mut everywhere = Vec::new();
    let mut global = global_hints(state);

    // too narrow for all of them: these go first, in this order, so help,
    // the palette and quitting stay
    for label in ["settings", "environments", "save", "send", "commands"] {
        if hints_width(&global) <= usize::from(area.width) {
            break;
        }

        global.retain(|hint| hint.label != label);
    }

    keys(&mut everywhere, bottom, global);

    frame.render_widget(Paragraph::new(Line::from(here)), top);
    frame.render_widget(Paragraph::new(Line::from(everywhere)), bottom);
}

/// the columns a list of hints takes in the footer
fn hints_width(hints: &[Hint]) -> usize {
    hints
        .iter()
        .map(|hint| hint.key.width() + hint.label.width() + 3)
        .sum()
}

/// the keys while a table cell is edited: from the name on to its value,
/// and from the value to the end of the row
pub(super) fn cell_hints(hints: &mut Hints, column: Column) {
    match column {
        Column::Name => {
            hints
                .one("next-cell", "to the value")
                .one("stop-edit", "done");
        }
        Column::Value => {
            hints.one("next-cell", "done");
        }
    }
}

/// the keys that work whatever has the focus; with a pop-up open, the
/// keys that finish it, and quitting
pub(crate) fn global_hints(state: &State) -> Vec<Hint> {
    match &state.popup {
        Some(Popup::Variables(editor)) if !editor.editing() => {
            let mut hints = Hints::here(state);

            hints.one("save", "save").one("close", "close");

            let mut list = hints.finish();

            list.extend(Hints::everywhere(state).one("quit", "quit").finish());

            return list;
        }
        Some(_) => return Hints::everywhere(state).one("quit", "quit").finish(),
        None => {}
    }

    Hints::everywhere(state)
        .one("send", "send")
        .one("save", "save")
        .one("environments", "environments")
        .one("settings", "settings")
        .one("palette", "commands")
        .one("help", "help")
        .one("quit", "quit")
        .finish()
}

/// the keys that work where the focus is; the ones that work everywhere
/// are in [`global_hints`]
pub(crate) fn hints(state: &State) -> Vec<Hint> {
    if let Some(popup) = &state.popup {
        return popup::hints(state, popup);
    }

    let mut hints = Hints::here(state);

    match state.focus {
        Focus::Sidebar => {
            hints
                .one("edit", "open")
                .one("new-request", "request")
                .one("new-folder", "folder")
                .one("new-collection", "collection")
                .one("rename", "rename")
                .one("delete", "delete")
                .pair("cut", "paste", "move");
        }
        Focus::Url => {
            hints.one("send", "send").one("focus.next", "next");
        }
        Focus::Method => {
            hints
                .pair("method.previous", "method.next", "change method")
                .one("focus.next", "next");
        }
        Focus::Request if state.request_typing() => match state.editing_column() {
            Some(column) => cell_hints(&mut hints, column),
            None => {
                hints
                    .one("stop-edit", "done")
                    .one("next-cell", "next field");
            }
        },
        Focus::Request => {
            hints.pair("tab.previous", "tab.next", "tab");

            let table = matches!(state.request_tab, RequestTab::Params | RequestTab::Headers)
                || (state.request_tab == RequestTab::Body && state.body.kind == BodyKind::Form);

            if table {
                hints
                    .one("row.add", "add")
                    .one("edit", "edit")
                    .one("row.toggle", "on/off")
                    .one("row.delete", "delete");
            } else if state.request_tab == RequestTab::Auth && state.auth.kind != AuthKind::None
                || state.request_tab == RequestTab::Body && state.body.kind != BodyKind::None
            {
                hints.one("edit", "edit");
            }

            if state.request_tab == RequestTab::Body && state.body.kind == BodyKind::Text {
                hints.one("content-type", "content type");
            }

            if matches!(state.request_tab, RequestTab::Body | RequestTab::Auth) {
                hints.one("kind", "type");
            }

            if request::credential_warning(state).is_some() {
                hints.one("make-secret", "make it a secret");
            }
        }
        Focus::Response => {
            hints
                .pair("tab.previous", "tab.next", "tab")
                .pair("up", "down", "scroll");

            if state.last_column() > 0 || state.column > 0 {
                hints.pair("left", "right", "sideways");
            }

            hints.one("save-body", "save");
        }
    }

    if state.in_flight.is_some() {
        hints.one("cancel", "cancel");
    }

    if matches!(state.outcome, Some(crate::app::Outcome::Response(_))) {
        hints.one("copy", "copy body");
    }

    hints.finish()
}

#[cfg(test)]
mod tests;
