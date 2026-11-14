//! the mouse: the view records what it drew where, and a click or the
//! wheel there becomes the actions a key would give, so the same checks
//! apply, such as asking before unsaved changes are lost

use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui_textarea::{CursorMove, TextArea};

use super::editor::Column;
use super::workspace::NodeKey;
use super::{Action, Focus, Popup, RequestTab, ResponseTab, State, request};

/// a text field a click can put the cursor in
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    Url,
    Body,

    /// the text field of a prompt or of the palette
    Popup,
}

/// a part of a table row
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cell {
    /// the on/off box
    Check,
    Name,
    Value,
}

/// what is drawn in a region of the screen
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// a pane, which a click gives the focus
    Pane(Focus),

    /// a text field, where a click puts the cursor
    Text(Field),

    /// the method, which a click changes
    Method,

    /// a tab title on a pane's border, by position
    Tab(Focus, usize),

    /// a tab title of the variable editor, by position
    PopupTab(usize),

    /// a request in the tab bar, by position
    Request(usize),

    /// a row of the sidebar
    Node(NodeKey),

    /// a row of a table, by position
    Row(usize, Cell),

    /// an item of a pop-up's list, by position
    Item(usize),

    /// an action, by its name in the keymap, such as a footer hint
    Run(String),

    /// the rest of a pop-up, where a click does nothing
    Popup,
}

/// a target and where it was drawn
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Region {
    pub(crate) area: Rect,
    pub(crate) target: Target,
}

impl State {
    /// forgets what was drawn, before a new frame is drawn
    pub(crate) fn clear_regions(&self) {
        self.regions.borrow_mut().clear();
    }

    /// records that `target` was drawn in `area`; later regions are on top
    pub(crate) fn region(&self, area: Rect, target: Target) {
        if area.width > 0 && area.height > 0 {
            self.regions.borrow_mut().push(Region { area, target });
        }
    }

    /// the topmost region at `at`
    fn region_at(&self, at: Position) -> Option<Region> {
        self.regions
            .borrow()
            .iter()
            .rev()
            .find(|r| r.area.contains(at))
            .cloned()
    }

    /// the pane at `at`, whatever is drawn on top of it
    fn pane_at(&self, at: Position) -> Option<Focus> {
        self.regions
            .borrow()
            .iter()
            .rev()
            .find_map(|r| match r.target {
                Target::Pane(focus) if r.area.contains(at) => Some(focus),
                _ => None,
            })
    }
}

/// pop-ups a click outside closes: nothing is lost by it, and the
/// variable editor still asks about unsaved changes
fn closes_outside(popup: &Popup) -> bool {
    matches!(
        popup,
        Popup::Help { .. }
            | Popup::Palette(_)
            | Popup::Tabs { .. }
            | Popup::Settings
            | Popup::Environments { .. }
            | Popup::Variables(_)
            | Popup::CollectionSettings { .. }
    )
}

/// pop-ups with a list the wheel moves through
fn scrolls(popup: &Popup) -> bool {
    match popup {
        Popup::Help { .. }
        | Popup::Palette(_)
        | Popup::Tabs { .. }
        | Popup::Environments { .. }
        | Popup::Welcome { .. }
        | Popup::SpecPreview(_)
        | Popup::SpecChoice { .. }
        | Popup::Review(_) => true,
        Popup::SaveRequest(dialog) => dialog.picking,
        Popup::Variables(editor) => !editor.editing(),
        _ => false,
    }
}

/// whether a mouse event can do anything: a left click or the wheel,
/// never a move, a drag or a release
pub(crate) fn mouse_acts(event: MouseEvent) -> bool {
    matches!(
        event.kind,
        MouseEventKind::Down(MouseButton::Left)
            | MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight
    )
}

/// the actions for a mouse event: a left click and the wheel do
/// something, every other event, such as moving the pointer, nothing
pub(crate) fn mouse_actions(state: &State, event: MouseEvent) -> Vec<Action> {
    let at = Position::new(event.column, event.row);

    match event.kind {
        MouseEventKind::Down(MouseButton::Left) => click(state, at),
        MouseEventKind::ScrollUp => wheel(state, at, true),
        MouseEventKind::ScrollDown => wheel(state, at, false),
        MouseEventKind::ScrollLeft if state.popup.is_none() && state.focus == Focus::Response => {
            vec![Action::Left]
        }
        MouseEventKind::ScrollRight if state.popup.is_none() && state.focus == Focus::Response => {
            vec![Action::Right]
        }
        _ => Vec::new(),
    }
}

/// where in a field a click landed, from the field's top left corner
fn offset(area: Rect, at: Position) -> (u16, u16) {
    (at.y - area.y, at.x - area.x)
}

fn click(state: &State, at: Position) -> Vec<Action> {
    let region = state.region_at(at);

    if let Some(popup) = &state.popup {
        return popup_click(state, popup, region, at);
    }

    let Some(Region { area, target }) = region else {
        return Vec::new();
    };

    match target {
        Target::Pane(focus) => vec![Action::FocusOn(focus)],
        Target::Text(field) => {
            let (row, col) = offset(area, at);

            vec![Action::Place { field, row, col }]
        }
        Target::Method => vec![Action::FocusOn(Focus::Method), Action::NextMethod],
        Target::Tab(focus, index) => vec![Action::FocusOn(focus), Action::ShowTab(index)],
        Target::Request(index) => vec![Action::RequestAt(index)],
        Target::Node(key) => vec![
            Action::FocusOn(Focus::Sidebar),
            Action::SelectNode(key),
            Action::Edit,
        ],
        Target::Row(index, cell) => {
            let again = state.focus == Focus::Request
                && state
                    .selected_row()
                    .is_some_and(|selected| selected == index);

            row_click(index, cell, again, Some(Focus::Request))
        }
        Target::Run(name) => super::keymap::action(&name).into_iter().collect(),
        Target::PopupTab(_) | Target::Item(_) | Target::Popup => Vec::new(),
    }
}

/// a click on a table row selects it; on the selected row it edits the
/// cell, and on the box it turns the row on or off
fn row_click(index: usize, cell: Cell, again: bool, focus: Option<Focus>) -> Vec<Action> {
    let mut actions: Vec<Action> = focus.map(Action::FocusOn).into_iter().collect();

    match cell {
        Cell::Check => actions.extend([Action::Pick(index), Action::ToggleRow]),
        Cell::Name if again => actions.push(Action::EditCell(Column::Name)),
        Cell::Value if again => actions.push(Action::EditCell(Column::Value)),
        Cell::Name | Cell::Value => actions.push(Action::Pick(index)),
    }

    actions
}

/// a click while a pop-up is open: inside it, on the footer's keys, or
/// anywhere else, which some pop-ups take as closing them
fn popup_click(state: &State, popup: &Popup, region: Option<Region>, at: Position) -> Vec<Action> {
    let within = state
        .regions
        .borrow()
        .iter()
        .any(|r| r.target == Target::Popup && r.area.contains(at));
    let Some(Region { area, target }) =
        region.filter(|r| within || matches!(r.target, Target::Run(_)))
    else {
        return if closes_outside(popup) {
            vec![Action::ClosePopup]
        } else {
            Vec::new()
        };
    };

    match (popup, target) {
        (Popup::Tabs { .. }, Target::Item(index)) => vec![Action::RequestAt(index)],
        (Popup::Variables(editor), Target::Row(index, cell)) => {
            let again = editor.table().selected == index && !editor.table().rows.is_empty();

            row_click(index, cell, again, None)
        }
        (_, Target::Item(index)) => vec![Action::Pick(index)],
        (_, Target::PopupTab(index)) => vec![Action::ShowTab(index)],
        (_, Target::Run(name)) => super::keymap::action(&name).into_iter().collect(),
        (_, Target::Text(Field::Popup)) => {
            let (row, col) = offset(area, at);

            vec![Action::Place {
                field: Field::Popup,
                row,
                col,
            }]
        }
        _ => Vec::new(),
    }
}

fn move_by(up: bool) -> Action {
    if up { Action::Up } else { Action::Down }
}

fn wheel(state: &State, at: Position, up: bool) -> Vec<Action> {
    if let Some(popup) = &state.popup {
        if !scrolls(popup) {
            return Vec::new();
        }

        // help is long, so it moves a few lines at a time
        let lines = if matches!(popup, Popup::Help { .. }) {
            3
        } else {
            1
        };

        return (0..lines).map(|_| move_by(up)).collect();
    }

    match state.pane_at(at) {
        Some(Focus::Response) => vec![Action::ScrollResponse(if up { -3 } else { 3 })],

        // the focus is not taken from a field being typed in
        Some(pane @ (Focus::Sidebar | Focus::Request)) if !state.request_typing() => {
            if state.focus == pane {
                vec![move_by(up)]
            } else {
                vec![Action::FocusOn(pane), move_by(up)]
            }
        }
        _ => Vec::new(),
    }
}

/// puts the cursor of `area` where a click landed, `row` and `col` counted
/// from the top left corner of where the field was drawn
pub(crate) fn place(area: &mut TextArea<'static>, row: u16, col: u16) {
    let (top, left) = area.scroll_offset();
    let at = area.screen_to_data(usize::from(row + top), usize::from(col + left));
    let row = u16::try_from(at.0).unwrap_or(u16::MAX);
    let col = u16::try_from(at.1).unwrap_or(u16::MAX);

    area.move_cursor(CursorMove::Jump(row, col));
}

/// applies what a click or the wheel asked for on the main screen
pub(super) fn apply(state: &mut State, action: Action) {
    match action {
        Action::FocusOn(focus) => {
            if state.focus != focus {
                state.stop_editing();
                state.focus = focus;
            }
        }
        Action::ShowTab(index) => match state.focus {
            Focus::Request => {
                if let Some(tab) = RequestTab::ALL.get(index) {
                    state.request_tab = *tab;
                }
            }
            Focus::Response => {
                if let Some(tab) = ResponseTab::ALL.get(index)
                    && *tab != state.response_tab
                {
                    state.response_tab = *tab;
                    state.reset_scroll();
                }
            }
            Focus::Sidebar | Focus::Method | Focus::Url => {}
        },
        Action::Pick(index) => {
            if let Some(table) = state.active_table() {
                table.commit();
                table.selected = index.min(table.rows.len().saturating_sub(1));
            }
        }
        Action::EditCell(column) => {
            if let Some(table) = state.active_table()
                && !table.rows.is_empty()
            {
                table.edit(column);
            }
        }
        Action::SelectNode(key) => state.sidebar.selected = Some(key),
        Action::Place { field, row, col } => request::place(state, field, row, col),
        Action::ScrollResponse(lines) => request::scroll_response(state, lines),
        _ => {}
    }
}
