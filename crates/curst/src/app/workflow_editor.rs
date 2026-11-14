//! making and changing a workflow: its name, then its steps, each a saved
//! request of the collection and the values to keep from its answer

use curst_core::model::{Capture, Extract, Item, LocalId, Step, Workflow};
use curst_core::vars::VarName;
use ratatui_textarea::TextArea;

use super::editor::{single_line, text_of};
use super::popup::{Pending, Popup};
use super::{Action, Change, Command, State};

/// one value a step keeps: the variable, and where in the answer it is
#[derive(Debug)]
pub(crate) struct EditCapture {
    pub(crate) variable: TextArea<'static>,
    pub(crate) from: TextArea<'static>,
}

impl EditCapture {
    fn new(variable: &str, from: &str) -> Self {
        Self {
            variable: single_line(variable, "variable"),
            from: single_line(from, "$.json.path, or header:Name"),
        }
    }
}

/// one step being edited
#[derive(Debug)]
pub(crate) struct EditStep {
    pub(crate) request: Vec<String>,
    pub(crate) keep: Vec<EditCapture>,
}

/// what a row of the editor is
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    Name,
    Request(usize),
    Capture(usize, usize),
    AddCapture(usize),
    AddStep,
}

impl Row {
    /// whether keys typed on the row go into a field
    pub(crate) fn typing(self) -> bool {
        matches!(self, Self::Name | Self::Capture(..))
    }
}

/// the editor
#[derive(Debug)]
pub(crate) struct WorkflowEditor {
    pub(crate) collection: LocalId,

    /// the workflow it changes, `None` for a new one
    pub(crate) index: Option<usize>,
    pub(crate) name: TextArea<'static>,
    pub(crate) steps: Vec<EditStep>,

    /// the selected row, and on a value's row, its field
    pub(crate) row: usize,
    pub(crate) column: usize,

    /// the request list open for a step, and the request selected in it
    pub(crate) picking: Option<(usize, usize)>,

    /// every request of the collection, by path, with how the list names it
    pub(crate) requests: Vec<(Vec<String>, String)>,

    /// what was typed and chosen when it opened, to know whether it changed
    opened: Typed,
}

/// the name, and each step's request and values, as typed
type Typed = (String, Vec<(Vec<String>, Vec<(String, String)>)>);

fn requests(items: &[Item], path: &[String], label: &str, out: &mut Vec<(Vec<String>, String)>) {
    for item in items {
        let mut here = path.to_vec();

        here.push(item.slug().to_owned());

        let named = if label.is_empty() {
            item.name().to_owned()
        } else {
            format!("{label} › {}", item.name())
        };

        match item {
            Item::Folder(folder) => requests(&folder.items, &here, &named, out),
            Item::Request(saved) => out.push((here, format!("{} {named}", saved.request.method))),
        }
    }
}

/// the text a value's "from" field shows for `from`
fn from_text(from: &Extract) -> String {
    match from {
        Extract::Json(path) => path.clone(),
        Extract::Header(name) => format!("header:{name}"),
    }
}

/// where a "from" field points: `header:Name` is a header, anything else
/// a JSON path
fn parse_from(text: &str) -> Option<Extract> {
    let text = text.trim();

    if text.is_empty() {
        return None;
    }

    let header = text
        .strip_prefix("header:")
        .or_else(|| text.strip_prefix("header "))
        .map(str::trim);

    Some(match header {
        Some(name) if !name.is_empty() => Extract::Header(name.to_owned()),
        Some(_) => return None,
        None => Extract::Json(text.to_owned()),
    })
}

impl WorkflowEditor {
    /// the rows, in order
    pub(crate) fn rows(&self) -> Vec<Row> {
        let mut rows = vec![Row::Name];

        for (step, edit) in self.steps.iter().enumerate() {
            rows.push(Row::Request(step));
            rows.extend((0..edit.keep.len()).map(|i| Row::Capture(step, i)));
            rows.push(Row::AddCapture(step));
        }

        rows.push(Row::AddStep);

        rows
    }

    pub(crate) fn current(&self) -> Row {
        self.rows().get(self.row).copied().unwrap_or(Row::Name)
    }

    /// how the list names the request at `path`
    pub(crate) fn label(&self, path: &[String]) -> Option<&str> {
        self.requests
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, label)| label.as_str())
    }

    pub(crate) fn field(&mut self) -> Option<&mut TextArea<'static>> {
        match self.current() {
            Row::Name => Some(&mut self.name),
            Row::Capture(step, i) => {
                let capture = &mut self.steps[step].keep[i];

                Some(if self.column == 0 {
                    &mut capture.variable
                } else {
                    &mut capture.from
                })
            }
            _ => None,
        }
    }

    /// the workflow the editor holds, or why it cannot be saved yet
    fn workflow(&self) -> Result<Workflow, String> {
        let name = text_of(&self.name).trim().to_owned();

        if name.is_empty() {
            return Err("give the workflow a name".into());
        }

        let mut steps = Vec::new();

        for (index, edit) in self.steps.iter().enumerate() {
            let mut keep = Vec::new();

            for capture in &edit.keep {
                let variable = text_of(&capture.variable);
                let from = text_of(&capture.from);

                // a row left empty is dropped
                if variable.trim().is_empty() && from.trim().is_empty() {
                    continue;
                }

                let variable = VarName::new(variable.trim()).ok_or_else(|| {
                    format!("step {}: {variable:?} is not a variable name", index + 1)
                })?;
                let from = parse_from(&from).ok_or_else(|| {
                    format!(
                        "step {}: say where {variable} is, as $.json.path or header:Name",
                        index + 1
                    )
                })?;

                keep.push(Capture { variable, from });
            }

            steps.push(Step {
                request: edit.request.clone(),
                keep,
            });
        }

        Ok(Workflow { name, steps })
    }

    /// what is typed and chosen, to compare
    fn typed(&self) -> Typed {
        (
            text_of(&self.name),
            self.steps
                .iter()
                .map(|step| {
                    let keep = step
                        .keep
                        .iter()
                        .map(|c| (text_of(&c.variable), text_of(&c.from)))
                        .collect();

                    (step.request.clone(), keep)
                })
                .collect(),
        )
    }

    /// whether it differs from what was opened
    pub(crate) fn is_changed(&self) -> bool {
        self.typed() != self.opened
    }
}

/// opens the editor on the workflow at `index`, or a new one
pub(crate) fn open(state: &mut State, collection: &LocalId, index: Option<usize>) {
    let Some(workspace) = state.workspace(collection) else {
        return;
    };

    let mut list = Vec::new();

    requests(&workspace.collection.items, &[], "", &mut list);

    let opened = index
        .and_then(|i| workspace.collection.workflows.get(i).cloned())
        .unwrap_or_default();
    let steps = opened
        .steps
        .iter()
        .map(|step| EditStep {
            request: step.request.clone(),
            keep: step
                .keep
                .iter()
                .map(|c| EditCapture::new(c.variable.as_str(), &from_text(&c.from)))
                .collect(),
        })
        .collect();

    let mut editor = WorkflowEditor {
        collection: collection.clone(),
        index,
        name: single_line(&opened.name, "name"),
        steps,
        row: 0,
        column: 0,
        picking: None,
        requests: list,
        opened: (String::new(), Vec::new()),
    };

    editor.opened = editor.typed();
    state.popup = Some(Popup::WorkflowEditor(Box::new(editor)));
}

/// a key in the editor
pub(crate) fn apply(
    state: &mut State,
    mut editor: Box<WorkflowEditor>,
    action: Action,
) -> Result<Vec<Command>, Action> {
    if let Some((step, selected)) = editor.picking {
        let handled = pick_key(&mut editor, step, selected, &action);

        state.popup = Some(Popup::WorkflowEditor(editor));

        return if handled { Ok(Vec::new()) } else { Err(action) };
    }

    let last = editor.rows().len() - 1;

    match action {
        Action::Up => {
            editor.row = editor.row.saturating_sub(1);
            editor.column = 0;
        }
        Action::Down => {
            editor.row = (editor.row + 1).min(last);
            editor.column = 0;
        }
        Action::Pick(row) => {
            editor.row = row.min(last);
            editor.column = 0;
        }
        Action::NextCell => match editor.current() {
            Row::Capture(..) if editor.column == 0 => editor.column = 1,
            _ => {
                editor.row = (editor.row + 1).min(last);
                editor.column = 0;
            }
        },
        Action::Type(key) => {
            if let Some(field) = editor.field() {
                field.input(key);
            }
        }
        Action::ConfirmPopup => confirm(&mut editor),
        Action::DeleteRow => delete_row(&mut editor),
        Action::Save => {
            return Ok(save(state, editor));
        }
        Action::Quit if editor.is_changed() => {
            state.popup = Some(Popup::confirm(
                "Quit without saving the workflow?",
                Pending::Quit(Some(Box::new(Popup::WorkflowEditor(editor)))),
            ));

            return Ok(Vec::new());
        }
        Action::ClosePopup if editor.is_changed() => {
            state.popup = Some(Popup::confirm(
                "Leave without saving the workflow?",
                Pending::Leave(Box::new(Popup::WorkflowEditor(editor))),
            ));

            return Ok(Vec::new());
        }
        Action::ClosePopup => return Ok(Vec::new()),
        other => {
            state.popup = Some(Popup::WorkflowEditor(editor));

            return Err(other);
        }
    }

    state.popup = Some(Popup::WorkflowEditor(editor));

    Ok(Vec::new())
}

/// Enter: on a step it opens the list of requests, on an add row it adds,
/// in a field it goes to the next row
fn confirm(editor: &mut WorkflowEditor) {
    let rows = editor.rows();

    match editor.current() {
        Row::Request(step) => {
            let selected = editor
                .requests
                .iter()
                .position(|(path, _)| *path == editor.steps[step].request)
                .unwrap_or(0);

            editor.picking = Some((step, selected));
        }
        Row::AddCapture(step) => {
            editor.steps[step].keep.push(EditCapture::new("", ""));
            editor.column = 0;
        }
        Row::AddStep => {
            let step = editor.steps.len();

            editor.steps.push(EditStep {
                request: Vec::new(),
                keep: Vec::new(),
            });
            editor.row = editor.rows().len() - 3;
            editor.picking = Some((step, 0));
        }
        Row::Name | Row::Capture(..) => {
            editor.row = (editor.row + 1).min(rows.len() - 1);
            editor.column = 0;
        }
    }
}

/// a key while the list of requests is open; `false` for one it has no
/// use for
fn pick_key(editor: &mut WorkflowEditor, step: usize, selected: usize, action: &Action) -> bool {
    let last = editor.requests.len().saturating_sub(1);

    match action {
        Action::Up => editor.picking = Some((step, selected.saturating_sub(1))),
        Action::Down => editor.picking = Some((step, (selected + 1).min(last))),
        Action::Pick(index) => {
            if let Some((path, _)) = editor.requests.get(*index) {
                editor.steps[step].request.clone_from(path);
            }

            editor.picking = None;
        }
        Action::ConfirmPopup => {
            if let Some((path, _)) = editor.requests.get(selected) {
                editor.steps[step].request.clone_from(path);
            }

            editor.picking = None;
        }
        Action::ClosePopup => editor.picking = None,
        _ => return false,
    }

    true
}

/// deletes a step, or a value it keeps
fn delete_row(editor: &mut WorkflowEditor) {
    match editor.current() {
        Row::Request(step) => {
            editor.steps.remove(step);
        }
        Row::Capture(step, i) => {
            editor.steps[step].keep.remove(i);
        }
        _ => return,
    }

    editor.row = editor.row.min(editor.rows().len() - 1);
    editor.column = 0;
}

/// saves the workflow, or says why it cannot be yet
fn save(state: &mut State, editor: Box<WorkflowEditor>) -> Vec<Command> {
    let workflow = match editor.workflow() {
        Ok(workflow) => workflow,
        Err(why) => {
            state.notice = Some(why);
            state.popup = Some(Popup::WorkflowEditor(editor));

            return Vec::new();
        }
    };

    if let Some(number) = workflow.steps.iter().position(|s| s.request.is_empty()) {
        state.notice = Some(format!("step {}: choose its request", number + 1));
        state.popup = Some(Popup::WorkflowEditor(editor));

        return Vec::new();
    }

    let Some(workspace) = state.workspace(&editor.collection) else {
        return Vec::new();
    };

    let mut workflows = workspace.collection.workflows.clone();

    match editor.index.filter(|i| *i < workflows.len()) {
        Some(index) => workflows[index] = workflow,
        None => workflows.push(workflow),
    }

    vec![Command::Store {
        collection: editor.collection.clone(),
        change: Change::SaveWorkflows(workflows),
    }]
}
