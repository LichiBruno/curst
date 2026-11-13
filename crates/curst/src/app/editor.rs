use curst_core::model::{Auth, Body, KeyValue, Variable};
use curst_core::secrets::Secret;
use ratatui::crossterm::event::KeyEvent;
use ratatui::style::{Modifier, Style};
use ratatui_textarea::TextArea;

pub(crate) fn single_line(text: &str, placeholder: &str) -> TextArea<'static> {
    let mut area = TextArea::new(vec![text.to_owned()]);
    area.set_placeholder_text(placeholder);
    area.set_cursor_line_style(Style::default());
    area.move_cursor(ratatui_textarea::CursorMove::End);

    area
}

/// a one-line field that shows `•` for every character, for a passphrase
/// or a secret value
pub(crate) fn masked(placeholder: &str) -> TextArea<'static> {
    let mut area = single_line("", placeholder);

    area.set_mask_char('•');

    area
}

pub(crate) fn multi_line(text: &str, placeholder: &str) -> TextArea<'static> {
    let mut area = TextArea::new(text.lines().map(ToOwned::to_owned).collect());
    area.set_placeholder_text(placeholder);
    area.set_cursor_line_style(Style::default());

    area
}

pub(crate) fn text_of(area: &TextArea<'_>) -> String {
    area.lines().join("\n")
}

pub(crate) fn show_cursor(area: &mut TextArea<'_>, visible: bool) {
    area.set_cursor_style(if visible {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Column {
    Name,
    Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) enabled: bool,

    /// a secret variable, whose value is not kept in the row
    pub(crate) secret: bool,

    /// the name the variable had when the table was filled, so a renamed
    /// secret can be moved rather than lost
    pub(crate) loaded_name: Option<String>,

    /// a new value typed for a secret, stored when the table is saved
    pub(crate) pending: Option<Secret>,
}

/// rows of name and value pairs, edited one cell at a time
#[derive(Debug, Default)]
pub(crate) struct Table {
    pub(crate) rows: Vec<Row>,
    pub(crate) selected: usize,
    pub(crate) editing: Option<(Column, TextArea<'static>)>,
}

impl Table {
    pub(crate) fn from_pairs(pairs: &[KeyValue]) -> Self {
        Self {
            rows: pairs
                .iter()
                .map(|p| Row {
                    name: p.name.clone(),
                    value: p.value.clone(),
                    enabled: p.enabled,
                    secret: false,
                    loaded_name: None,
                    pending: None,
                })
                .collect(),
            selected: 0,
            editing: None,
        }
    }

    pub(crate) fn from_variables(variables: &[Variable]) -> Self {
        Self {
            rows: variables
                .iter()
                .map(|v| Row {
                    name: v.name.as_str().to_owned(),
                    value: v.value.clone(),
                    enabled: v.enabled,
                    secret: v.secret,
                    loaded_name: Some(v.name.as_str().to_owned()),
                    pending: None,
                })
                .collect(),
            selected: 0,
            editing: None,
        }
    }

    pub(crate) fn pairs(&self) -> Vec<KeyValue> {
        self.rows
            .iter()
            .map(|row| KeyValue {
                name: row.name.trim().to_owned(),
                value: row.value.clone(),
                enabled: row.enabled,
            })
            .collect()
    }

    pub(crate) fn select_next(&mut self) {
        if self.selected + 1 < self.rows.len() {
            self.selected += 1;
        }
    }

    pub(crate) fn select_previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub(crate) fn add_row(&mut self) {
        self.commit();
        self.rows.push(Row {
            name: String::new(),
            value: String::new(),
            enabled: true,
            secret: false,
            loaded_name: None,
            pending: None,
        });
        self.selected = self.rows.len() - 1;
        self.edit(Column::Name);
    }

    pub(crate) fn delete_selected(&mut self) {
        if self.selected < self.rows.len() {
            self.rows.remove(self.selected);
            self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        }
    }

    pub(crate) fn toggle_selected(&mut self) {
        if let Some(row) = self.rows.get_mut(self.selected) {
            row.enabled = !row.enabled;
        }
    }

    pub(crate) fn edit(&mut self, column: Column) {
        let Some(row) = self.rows.get(self.selected) else {
            return;
        };

        // a secret's value is typed blind, and never shown back
        if column == Column::Value && row.secret {
            self.editing = Some((
                column,
                masked("new value, or leave it empty to keep the old one"),
            ));

            return;
        }

        let (text, placeholder) = match column {
            Column::Name => (&row.name, "name"),
            Column::Value => (&row.value, "value"),
        };
        self.editing = Some((column, single_line(text, placeholder)));
    }

    /// moves from the name to the value, and finishes after the value
    pub(crate) fn next_cell(&mut self) {
        let column = self.editing.as_ref().map(|(column, _)| *column);

        self.commit();

        if column == Some(Column::Name) {
            self.edit(Column::Value);
        }
    }

    /// writes the cell being edited back into its row and stops editing,
    /// dropping the row when both cells are empty
    pub(crate) fn commit(&mut self) {
        let Some((column, area)) = self.editing.take() else {
            return;
        };

        let Some(row) = self.rows.get_mut(self.selected) else {
            return;
        };

        let text = text_of(&area);

        match column {
            Column::Name => row.name = text,
            Column::Value if row.secret => {
                if !text.is_empty() {
                    row.pending = Some(Secret::new(text));
                }
            }
            Column::Value => row.value = text,
        }

        if row.name.trim().is_empty() && row.value.is_empty() {
            self.delete_selected();
        }
    }

    /// makes the selected row a secret; a value it had becomes the secret's
    /// value, and leaves the row
    pub(crate) fn make_secret(&mut self) {
        self.commit();

        if let Some(row) = self.rows.get_mut(self.selected)
            && !row.secret
        {
            row.secret = true;

            if !row.value.is_empty() {
                row.pending = Some(Secret::new(std::mem::take(&mut row.value)));
            }
        }
    }

    /// turns the selected secret row back into a plain one holding `value`
    pub(crate) fn make_plain(&mut self, value: String) {
        self.commit();

        if let Some(row) = self.rows.get_mut(self.selected) {
            row.secret = false;
            row.pending = None;
            row.value = value;
        }
    }

    pub(crate) fn input(&mut self, key: KeyEvent) {
        if let Some((_, area)) = &mut self.editing {
            area.input(key);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum BodyKind {
    #[default]
    None,
    Json,
    Text,
    Form,
}

impl BodyKind {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::None => Self::Json,
            Self::Json => Self::Text,
            Self::Text => Self::Form,
            Self::Form => Self::None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Json => "JSON",
            Self::Text => "text",
            Self::Form => "form",
        }
    }
}

/// a multi-line body field holding `text` exactly, a trailing newline
/// included
pub(crate) fn body_text(text: &str) -> TextArea<'static> {
    let mut area = TextArea::new(text.split('\n').map(ToOwned::to_owned).collect());

    area.set_placeholder_text("press Enter to write the body");
    area.set_cursor_line_style(Style::default());

    area
}

/// the text field of the body tab that receives keys
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BodyField {
    Text,
    ContentType,
}

const TEXT_TYPE: &str = "text/plain";

#[derive(Debug)]
pub(crate) struct BodyEditor {
    pub(crate) kind: BodyKind,
    pub(crate) text: TextArea<'static>,
    pub(crate) content_type: TextArea<'static>,
    pub(crate) form: Table,
    pub(crate) typing: Option<BodyField>,
}

impl Default for BodyEditor {
    fn default() -> Self {
        Self {
            kind: BodyKind::None,
            text: multi_line("", "press Enter to write the body"),
            content_type: single_line(TEXT_TYPE, TEXT_TYPE),
            form: Table::default(),
            typing: None,
        }
    }
}

impl BodyEditor {
    pub(crate) fn from_body(body: &Body) -> Self {
        let mut editor = Self::default();

        match body {
            Body::None => {}
            Body::Json(text) => {
                editor.kind = BodyKind::Json;
                editor.text = body_text(text);
            }
            Body::Text { content_type, text } => {
                editor.kind = BodyKind::Text;
                editor.text = body_text(text);
                editor.content_type = single_line(content_type, TEXT_TYPE);
            }
            Body::Form(fields) => {
                editor.kind = BodyKind::Form;
                editor.form = Table::from_pairs(fields);
            }
        }

        editor
    }

    pub(crate) fn body(&self) -> Body {
        match self.kind {
            BodyKind::None => Body::None,
            BodyKind::Json => Body::Json(text_of(&self.text)),
            BodyKind::Text => {
                let content_type = text_of(&self.content_type).trim().to_owned();

                Body::Text {
                    content_type: if content_type.is_empty() {
                        TEXT_TYPE.into()
                    } else {
                        content_type
                    },
                    text: text_of(&self.text),
                }
            }
            BodyKind::Form => Body::Form(self.form.pairs()),
        }
    }

    /// the field keys go to, if any
    pub(crate) fn field(&mut self) -> Option<&mut TextArea<'static>> {
        match self.typing? {
            BodyField::Text => Some(&mut self.text),
            BodyField::ContentType => Some(&mut self.content_type),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AuthKind {
    #[default]
    None,
    Basic,
    Bearer,
}

impl AuthKind {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::None => Self::Basic,
            Self::Basic => Self::Bearer,
            Self::Bearer => Self::None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Basic => "basic",
            Self::Bearer => "bearer token",
        }
    }

    pub(crate) fn field_count(self) -> usize {
        match self {
            Self::None => 0,
            Self::Basic => 2,
            Self::Bearer => 1,
        }
    }
}

#[derive(Debug)]
pub(crate) struct AuthEditor {
    pub(crate) kind: AuthKind,
    pub(crate) username: TextArea<'static>,
    pub(crate) password: TextArea<'static>,
    pub(crate) token: TextArea<'static>,
    pub(crate) selected: usize,
    pub(crate) typing: bool,
}

impl Default for AuthEditor {
    fn default() -> Self {
        let mut password = single_line("", "password");
        password.set_mask_char('•');

        Self {
            kind: AuthKind::None,
            username: single_line("", "user name"),
            password,
            token: single_line("", "token"),
            selected: 0,
            typing: false,
        }
    }
}

impl AuthEditor {
    pub(crate) fn from_auth(auth: &Auth) -> Self {
        let mut editor = Self::default();

        match auth {
            Auth::None => {}
            Auth::Basic { username, password } => {
                editor.kind = AuthKind::Basic;
                editor.username = single_line(username, "user name");
                editor.password = single_line(password, "password");
                editor.password.set_mask_char('•');
            }
            Auth::Bearer { token } => {
                editor.kind = AuthKind::Bearer;
                editor.token = single_line(token, "token");
            }
        }

        editor
    }

    pub(crate) fn auth(&self) -> Auth {
        match self.kind {
            AuthKind::None => Auth::None,
            AuthKind::Basic => Auth::Basic {
                username: text_of(&self.username),
                password: text_of(&self.password),
            },
            AuthKind::Bearer => Auth::Bearer {
                token: text_of(&self.token),
            },
        }
    }

    pub(crate) fn fields(&self) -> Vec<(&'static str, &TextArea<'static>)> {
        match self.kind {
            AuthKind::None => Vec::new(),
            AuthKind::Basic => vec![("User name", &self.username), ("Password", &self.password)],
            AuthKind::Bearer => vec![("Token", &self.token)],
        }
    }

    pub(crate) fn selected_field(&mut self) -> Option<&mut TextArea<'static>> {
        match (self.kind, self.selected) {
            (AuthKind::Basic, 0) => Some(&mut self.username),
            (AuthKind::Basic, 1) => Some(&mut self.password),
            (AuthKind::Bearer, 0) => Some(&mut self.token),
            _ => None,
        }
    }

    pub(crate) fn cycle_kind(&mut self) {
        self.kind = self.kind.next();
        self.selected = 0;
        self.typing = false;
    }

    pub(crate) fn select_next(&mut self) {
        if self.selected + 1 < self.kind.field_count() {
            self.selected += 1;
        }
    }

    pub(crate) fn select_previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    fn type_text(table: &mut Table, text: &str) {
        for c in text.chars() {
            table.input(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    #[test]
    fn add_and_fill_a_row() {
        let mut table = Table::default();

        table.add_row();
        type_text(&mut table, "Accept");
        table.next_cell();
        type_text(&mut table, "text/html");
        table.next_cell();

        assert!(table.editing.is_none());
        assert_eq!(table.pairs(), vec![KeyValue::new("Accept", "text/html")]);
    }

    #[test]
    fn empty_rows_are_dropped_on_commit() {
        let mut table = Table::default();

        table.add_row();
        table.commit();

        assert_eq!(table.rows, Vec::new());
    }

    #[test]
    fn toggle_select_and_delete() {
        let mut table = Table::default();

        for name in ["a", "b", "c"] {
            table.add_row();
            type_text(&mut table, name);
            table.commit();
        }

        table.select_previous();
        table.toggle_selected();

        assert!(!table.rows[1].enabled);

        table.select_next();
        table.select_next();

        assert_eq!(table.selected, 2);

        table.delete_selected();

        assert_eq!(table.selected, 1);

        table.delete_selected();
        table.delete_selected();
        table.delete_selected();

        assert_eq!(table.rows, Vec::new());

        table.edit(Column::Name);

        assert!(table.editing.is_none());
    }

    #[test]
    fn body_kinds_build_the_model_body() {
        let mut body = BodyEditor::default();

        assert_eq!(body.body(), Body::None);

        body.kind = body.kind.next();
        body.text.insert_str("{}");

        assert_eq!(body.body(), Body::Json("{}".into()));

        body.kind = body.kind.next();

        assert_eq!(
            body.body(),
            Body::Text {
                content_type: "text/plain".into(),
                text: "{}".into()
            }
        );

        body.content_type = single_line("  application/xml ", "");

        assert_eq!(
            body.body(),
            Body::Text {
                content_type: "application/xml".into(),
                text: "{}".into()
            }
        );

        body.content_type = single_line("", "");

        assert!(matches!(
            body.body(),
            Body::Text { content_type, .. } if content_type == "text/plain"
        ));

        body.kind = body.kind.next();

        assert_eq!(body.body(), Body::Form(vec![]));
        assert_eq!(body.kind.next(), BodyKind::None);
    }

    #[test]
    fn editors_load_what_they_produce() {
        let mut off = KeyValue::new("b", "2");
        off.enabled = false;

        let pairs = vec![KeyValue::new("a", "1"), off];

        assert_eq!(Table::from_pairs(&pairs).pairs(), pairs);

        for body in [
            Body::None,
            Body::Json("{\n}\n".into()),
            Body::Text {
                content_type: "text/csv".into(),
                text: "a,b".into(),
            },
            Body::Form(pairs.clone()),
        ] {
            assert_eq!(BodyEditor::from_body(&body).body(), body);
        }

        for auth in [
            Auth::None,
            Auth::Basic {
                username: "u".into(),
                password: "p".into(),
            },
            Auth::Bearer { token: "t".into() },
        ] {
            assert_eq!(AuthEditor::from_auth(&auth).auth(), auth);
        }
    }

    #[test]
    fn the_typed_body_field_takes_the_keys() {
        let mut body = BodyEditor::default();

        assert!(body.field().is_none());

        body.typing = Some(BodyField::ContentType);

        assert_eq!(body.field().map(|f| text_of(f)), Some("text/plain".into()));

        body.typing = Some(BodyField::Text);

        assert_eq!(body.field().map(|f| text_of(f)), Some(String::new()));
    }

    #[test]
    fn auth_kinds_build_the_model_auth() {
        let mut auth = AuthEditor::default();

        assert_eq!(auth.auth(), Auth::None);
        assert!(auth.selected_field().is_none());

        auth.cycle_kind();
        auth.selected_field().unwrap().insert_str("me");
        auth.select_next();
        auth.selected_field().unwrap().insert_str("pw");
        auth.select_next();

        assert_eq!(auth.selected, 1);
        assert_eq!(
            auth.auth(),
            Auth::Basic {
                username: "me".into(),
                password: "pw".into()
            }
        );

        auth.cycle_kind();
        auth.selected_field().unwrap().insert_str("t");

        assert_eq!(auth.auth(), Auth::Bearer { token: "t".into() });

        auth.select_previous();

        assert_eq!(auth.fields().len(), 1);
    }
}
