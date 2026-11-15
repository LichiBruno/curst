//! the vim keymap's modes: a field is typed in (insert) or moved around
//! and changed (normal); Esc goes from insert to normal, and from normal
//! out of the field, back to moving between rows and tabs. the URL is a
//! field whenever it has the focus, in normal mode until `i`
//!
//! the panes are reached with Ctrl+h/j/k/l, or vim's Ctrl+w h/j/k/l

use ratatui_textarea::{CursorMove, TextArea};

use super::{Action, BodyField, BodyKind, Command, Focus, RequestTab, State};

/// what a vim key does in a field
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Vim {
    Insert,
    Append,
    InsertStart,
    AppendEnd,
    Normal,
    Leave,
    Left,
    Right,
    Word,
    Back,
    WordEnd,
    LineStart,
    FirstChar,
    LineEnd,
    Up,
    Down,
    DeleteChar,
    DeleteBack,
    DeleteToEnd,
    ChangeToEnd,
    DeleteLine,
    ChangeLine,
    DeleteWord,
    ChangeWord,
    ChangeInnerWord,
    Undo,
}

impl Vim {
    /// the key named `vim.<name>` in the keymap
    pub(crate) fn named(name: &str) -> Option<Self> {
        Some(match name {
            "insert" => Self::Insert,
            "append" => Self::Append,
            "insert-start" => Self::InsertStart,
            "append-end" => Self::AppendEnd,
            "normal" => Self::Normal,
            "leave" => Self::Leave,
            "left" => Self::Left,
            "right" => Self::Right,
            "word" => Self::Word,
            "back" => Self::Back,
            "word-end" => Self::WordEnd,
            "line-start" => Self::LineStart,
            "first-char" => Self::FirstChar,
            "line-end" => Self::LineEnd,
            "up" => Self::Up,
            "down" => Self::Down,
            "delete-char" => Self::DeleteChar,
            "delete-back" => Self::DeleteBack,
            "delete-to-end" => Self::DeleteToEnd,
            "change-to-end" => Self::ChangeToEnd,
            "delete-line" => Self::DeleteLine,
            "change-line" => Self::ChangeLine,
            "delete-word" => Self::DeleteWord,
            "change-word" => Self::ChangeWord,
            "change-inner-word" => Self::ChangeInnerWord,
            "undo" => Self::Undo,
            _ => return None,
        })
    }
}

/// a direction to move the focus in
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pane {
    Left,
    Down,
    Up,
    Right,
}

impl Pane {
    pub(crate) fn named(name: &str) -> Option<Self> {
        Some(match name {
            "left" => Self::Left,
            "down" => Self::Down,
            "up" => Self::Up,
            "right" => Self::Right,
            _ => return None,
        })
    }
}

impl State {
    /// whether the vim keymap is in use, with its modes
    pub(crate) fn vim(&self) -> bool {
        self.keys_preset == "vim"
    }

    /// whether the focus is in a field: the URL, or a field of the request
    /// being edited
    pub(crate) fn in_field(&self) -> bool {
        match self.focus {
            Focus::Url => true,
            Focus::Request => self.request_typing(),
            Focus::Sidebar | Focus::Method | Focus::Response => false,
        }
    }

    /// the field the focus is in, if it is one
    fn field_mut(&mut self) -> Option<&mut TextArea<'static>> {
        match self.focus {
            Focus::Url => Some(&mut self.url),
            Focus::Request => match self.request_tab {
                RequestTab::Body if self.body.kind != BodyKind::Form => self.body.field(),
                RequestTab::Auth if self.auth.typing => self.auth.selected_field(),
                RequestTab::Auth => None,
                RequestTab::Params | RequestTab::Headers | RequestTab::Body => self
                    .active_table()
                    .and_then(|table| table.editing.as_mut().map(|(_, area)| area)),
            },
            Focus::Sidebar | Focus::Method | Focus::Response => None,
        }
    }

    /// whether the field is the body's text, the only one with lines
    fn in_body_text(&self) -> bool {
        self.focus == Focus::Request
            && self.request_tab == RequestTab::Body
            && self.body.typing == Some(BodyField::Text)
    }
}

/// what a character is, for word motions: part of a word, a run of
/// punctuation, or space
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    Punctuation,
    Word,
}

fn class(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Punctuation
    }
}

/// the columns of the word, punctuation run or space under `col` in `line`,
/// as `start..end`
fn word_at(line: &str, col: usize) -> (usize, usize) {
    let chars: Vec<char> = line.chars().collect();

    if chars.is_empty() {
        return (0, 0);
    }

    let col = col.min(chars.len() - 1);
    let kind = class(chars[col]);
    let start = (0..col)
        .rev()
        .take_while(|i| class(chars[*i]) == kind)
        .last()
        .unwrap_or(col);
    let end = (col..chars.len())
        .take_while(|i| class(chars[*i]) == kind)
        .last()
        .map_or(col, |i| i + 1);

    (start, end)
}

/// the column the rest of the word under the cursor ends at, as `cw` and
/// `ce` see it
fn word_end(line: &str, col: usize) -> usize {
    let chars: Vec<char> = line.chars().collect();

    if col >= chars.len() {
        return col;
    }

    word_at(line, col).1
}

/// the current line and the cursor's place in it
fn here(area: &TextArea<'_>) -> (String, usize, usize) {
    let cursor = area.cursor();
    let line = area.lines().get(cursor.0).cloned().unwrap_or_default();

    (line, cursor.0, cursor.1)
}

/// moves the cursor to `col` of row `row`
fn jump(area: &mut TextArea<'_>, row: usize, col: usize) {
    area.move_cursor(CursorMove::Jump(
        u16::try_from(row).unwrap_or(u16::MAX),
        u16::try_from(col).unwrap_or(u16::MAX),
    ));
}

/// deletes the line the cursor is on; in a single line, its text
fn delete_line(area: &mut TextArea<'_>) {
    let rows = area.lines().len();
    let row = area.cursor().0;

    area.move_cursor(CursorMove::Head);
    area.delete_line_by_end();

    if rows > 1 {
        if row + 1 < rows {
            area.delete_next_char();
        } else {
            area.delete_char();
        }

        area.move_cursor(CursorMove::Head);
    }
}

/// applies a motion or a change to the text of `area`; `true` when it
/// leaves the field in insert mode
fn on_text(area: &mut TextArea<'_>, key: Vim) -> bool {
    match key {
        Vim::Left => area.move_cursor(CursorMove::Back),
        Vim::Right => area.move_cursor(CursorMove::Forward),
        Vim::Word => area.move_cursor(CursorMove::WordForward),
        Vim::Back => area.move_cursor(CursorMove::WordBack),
        Vim::WordEnd => area.move_cursor(CursorMove::WordEnd),
        Vim::LineStart => area.move_cursor(CursorMove::Head),
        Vim::LineEnd => area.move_cursor(CursorMove::End),
        Vim::FirstChar => {
            let (line, row, _) = here(area);
            let first = line.chars().take_while(|c| c.is_whitespace()).count();

            jump(area, row, first);
        }
        Vim::DeleteChar => {
            area.delete_next_char();
        }
        Vim::DeleteBack => {
            area.delete_char();
        }
        Vim::DeleteToEnd => {
            area.delete_line_by_end();
        }
        Vim::ChangeToEnd => {
            area.delete_line_by_end();

            return true;
        }
        Vim::DeleteLine => delete_line(area),
        Vim::ChangeLine => {
            area.move_cursor(CursorMove::Head);
            area.delete_line_by_end();

            return true;
        }
        Vim::DeleteWord => {
            area.delete_next_word();
        }
        Vim::ChangeWord => {
            let (line, _, col) = here(area);
            let end = word_end(&line, col);

            area.delete_str(end.saturating_sub(col));

            return true;
        }
        Vim::ChangeInnerWord => {
            let (line, row, col) = here(area);
            let (start, end) = word_at(&line, col);

            jump(area, row, start);
            area.delete_str(end - start);

            return true;
        }
        Vim::Undo => {
            area.undo();
        }
        Vim::Insert => return true,
        Vim::Append => {
            area.move_cursor(CursorMove::Forward);

            return true;
        }
        Vim::InsertStart => {
            area.move_cursor(CursorMove::Head);

            return true;
        }
        Vim::AppendEnd => {
            area.move_cursor(CursorMove::End);

            return true;
        }
        Vim::Normal | Vim::Leave | Vim::Up | Vim::Down => {}
    }

    false
}

/// a vim key in a field
pub(crate) fn apply(state: &mut State, key: Vim) -> Vec<Command> {
    match key {
        Vim::Normal => {
            state.vim_insert = false;

            // as vim does, the cursor steps back onto the last character
            if let Some(area) = state.field_mut()
                && area.cursor().1 > 0
            {
                area.move_cursor(CursorMove::Back);
            }

            return Vec::new();
        }
        Vim::Leave if state.focus == Focus::Url => return super::apply(state, Action::Cancel),
        Vim::Leave => {
            state.stop_editing();

            return Vec::new();
        }

        // the body's text has lines; any other field is one row of a list,
        // which j and k leave for the row below or above
        Vim::Up | Vim::Down if state.in_body_text() => {
            if let Some(area) = state.field_mut() {
                area.move_cursor(if key == Vim::Up {
                    CursorMove::Up
                } else {
                    CursorMove::Down
                });
            }

            return Vec::new();
        }
        Vim::Up | Vim::Down if state.focus == Focus::Request => {
            state.stop_editing();

            let step = if key == Vim::Up {
                Action::Up
            } else {
                Action::Down
            };

            return super::apply(state, step);
        }
        Vim::Up | Vim::Down => return Vec::new(),
        _ => {}
    }

    // a key that types, outside a field, opens it first
    let opens = matches!(
        key,
        Vim::Insert | Vim::Append | Vim::InsertStart | Vim::AppendEnd
    );

    if opens && !state.in_field() && state.focus == Focus::Request {
        super::request::edit(state);
    }

    let insert = state.field_mut().is_some_and(|area| on_text(area, key));

    if insert {
        state.vim_insert = true;
    }

    Vec::new()
}

/// moves the focus to the pane in `direction`, as the screen lays them
/// out: the sidebar on the left; the method, the URL, the request and the
/// response from the top down on the right
pub(crate) fn pane(state: &mut State, direction: Pane) {
    if state.popup.is_some() {
        return;
    }

    let next = match (state.focus, direction) {
        (Focus::Url, Pane::Left) => Focus::Method,
        (Focus::Method | Focus::Request | Focus::Response, Pane::Left) => Focus::Sidebar,
        (Focus::Sidebar | Focus::Method, Pane::Right) | (Focus::Request, Pane::Up) => Focus::Url,
        (Focus::Url | Focus::Method, Pane::Down) | (Focus::Response, Pane::Up) => Focus::Request,
        (Focus::Request, Pane::Down) => Focus::Response,
        (focus, _) => focus,
    };

    if next != state.focus {
        state.stop_editing();
        state.focus = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(text: &str, col: usize) -> TextArea<'static> {
        let mut area = super::super::editor::single_line(text, "");

        jump(&mut area, 0, col);

        area
    }

    fn text(area: &TextArea<'_>) -> String {
        area.lines().join("\n")
    }

    #[test]
    fn words_split_at_space_and_punctuation() {
        assert_eq!(word_at("api/v1 users", 1), (0, 3));
        assert_eq!(word_at("api/v1 users", 3), (3, 4));
        assert_eq!(word_at("api/v1 users", 9), (7, 12));
        assert_eq!(word_at("", 0), (0, 0));
        assert_eq!(word_end("api/v1", 1), 3);
    }

    #[test]
    fn changes_keep_the_rest_of_the_text() {
        let mut url = area("https://api.example.com/v1", 9);

        assert!(on_text(&mut url, Vim::ChangeInnerWord));
        assert_eq!(text(&url), "https://.example.com/v1");

        let mut url = area("https://api.example.com/v1", 8);

        assert!(on_text(&mut url, Vim::ChangeWord));
        assert_eq!(text(&url), "https://.example.com/v1");

        let mut url = area("{{base}}/users", 8);

        assert!(!on_text(&mut url, Vim::DeleteToEnd));
        assert_eq!(text(&url), "{{base}}");

        let mut url = area("x", 0);

        assert!(!on_text(&mut url, Vim::DeleteLine));
        assert_eq!(text(&url), "");
        assert!(!on_text(&mut url, Vim::Undo));
        assert_eq!(text(&url), "x");
    }

    #[test]
    fn deleting_a_line_of_the_body_joins_the_rest() {
        let mut body = TextArea::from(["{", "  \"a\": 1", "}"]);

        jump(&mut body, 1, 3);
        delete_line(&mut body);

        assert_eq!(text(&body), "{\n}");
        assert_eq!(body.cursor(), (1, 0));

        jump(&mut body, 1, 0);
        delete_line(&mut body);

        assert_eq!(text(&body), "{");
    }
}
