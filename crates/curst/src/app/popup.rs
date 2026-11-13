//! pop-ups over the main screen: text prompts, yes/no questions, the
//! environment picker and the variable editor
//!
//! while one is open it receives every key but `Ctrl+C`

use std::path::PathBuf;

use curst_core::model::{EnvId, Environment, KeyValue, LocalId, Request, Variable};
use curst_core::vars::VarName;
use ratatui_textarea::TextArea;

use super::editor::{Column, Table, single_line, text_of};
use super::workspace::NodeKey;
use super::{Action, Change, Command, State, step};

/// what a text prompt is for
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Purpose {
    SaveBody,
    SaveAs(NodeKey),
    NewRequest(NodeKey),
    NewFolder(NodeKey),
    NewCollection,
    Rename(NodeKey),
    NewEnvironment(LocalId),
    RenameEnvironment(LocalId, EnvId),
}

impl Purpose {
    pub(crate) fn title(&self) -> &'static str {
        match self {
            Self::SaveBody => "Save body to file",
            Self::SaveAs(_) => "Save request as",
            Self::NewRequest(_) => "New request",
            Self::NewFolder(_) => "New folder",
            Self::NewCollection => "New collection",
            Self::Rename(_) => "Rename",
            Self::NewEnvironment(_) => "New environment",
            Self::RenameEnvironment(..) => "Rename environment",
        }
    }
}

/// what a yes/no question does on yes
#[derive(Debug)]
pub(crate) enum Pending {
    Open(NodeKey),
    Delete(NodeKey),
    DeleteEnvironment(LocalId, EnvId),
    /// quitting, with the pop-up to show again on no
    Quit(Option<Box<Popup>>),

    /// closing the variable editor; on no it comes back as it was
    CloseVariables(Box<VariablesEditor>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VarTab {
    Environment,
    Collection,
    Globals,
}

impl VarTab {
    pub(crate) const ALL: [Self; 3] = [Self::Environment, Self::Collection, Self::Globals];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Environment => "Environment",
            Self::Collection => "Collection",
            Self::Globals => "Globals",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Environment => 0,
            Self::Collection => 1,
            Self::Globals => 2,
        }
    }
}

/// the variables of one environment, one collection and the globals, each
/// in a table
#[derive(Debug)]
pub(crate) struct VariablesEditor {
    pub(crate) collection: LocalId,
    pub(crate) environment: Option<Environment>,
    pub(crate) tab: VarTab,
    pub(crate) tables: [Table; 3],
    baseline: [Vec<KeyValue>; 3],
}

fn pairs_of(variables: &[Variable]) -> Vec<KeyValue> {
    variables
        .iter()
        .map(|v| KeyValue {
            name: v.name.as_str().to_owned(),
            value: v.value.clone(),
            enabled: v.enabled,
        })
        .collect()
}

impl VariablesEditor {
    pub(crate) fn new(
        collection: LocalId,
        environment: Option<Environment>,
        collection_vars: &[Variable],
        globals: &[Variable],
    ) -> Self {
        let baseline = [
            environment
                .as_ref()
                .map_or_else(Vec::new, |e| pairs_of(&e.variables)),
            pairs_of(collection_vars),
            pairs_of(globals),
        ];
        let tables = baseline.clone().map(|pairs| Table::from_pairs(&pairs));
        let tab = if environment.is_some() {
            VarTab::Environment
        } else {
            VarTab::Collection
        };

        Self {
            collection,
            environment,
            tab,
            tables,
            baseline,
        }
    }

    /// the tabs shown: the environment one only when an environment is
    /// being edited
    pub(crate) fn tabs(&self) -> Vec<VarTab> {
        VarTab::ALL
            .into_iter()
            .filter(|t| *t != VarTab::Environment || self.environment.is_some())
            .collect()
    }

    pub(crate) fn table(&self) -> &Table {
        &self.tables[self.tab.index()]
    }

    fn table_mut(&mut self) -> &mut Table {
        &mut self.tables[self.tab.index()]
    }

    pub(crate) fn editing(&self) -> bool {
        self.table().editing.is_some()
    }

    fn changed(&self, tab: VarTab) -> bool {
        self.tables[tab.index()].pairs() != self.baseline[tab.index()]
    }

    pub(crate) fn is_changed(&self) -> bool {
        VarTab::ALL.into_iter().any(|t| self.changed(t))
    }

    /// the variables of a tab, or the row and reason of the first bad one
    fn variables(&self, tab: VarTab) -> Result<Vec<Variable>, (usize, String)> {
        let mut variables: Vec<Variable> = Vec::new();

        for (row, pair) in self.tables[tab.index()].pairs().into_iter().enumerate() {
            let Some(name) = VarName::new(&pair.name) else {
                return Err((
                    row,
                    format!(
                        "{:?} is not a valid name: use letters, digits, _ - and .",
                        pair.name
                    ),
                ));
            };

            if variables.iter().any(|v| v.name == name) {
                return Err((row, format!("{name} is defined twice")));
            }

            variables.push(Variable {
                name,
                value: pair.value,
                enabled: pair.enabled,
            });
        }

        Ok(variables)
    }
}

#[derive(Debug)]
pub(crate) enum Popup {
    Prompt {
        purpose: Purpose,
        input: Box<TextArea<'static>>,
    },
    Confirm {
        message: String,
        pending: Pending,
    },

    /// `selected` 0 is "no environment", then one per environment
    Environments {
        collection: LocalId,
        selected: usize,
    },
    Variables(Box<VariablesEditor>),
}

impl Popup {
    pub(crate) fn prompt(purpose: Purpose, text: &str) -> Self {
        let placeholder = if purpose == Purpose::SaveBody {
            "file name"
        } else {
            "name"
        };

        Self::Prompt {
            purpose,
            input: Box::new(single_line(text, placeholder)),
        }
    }

    pub(crate) fn confirm(message: impl Into<String>, pending: Pending) -> Self {
        Self::Confirm {
            message: message.into(),
            pending,
        }
    }
}

/// handles `action` while a pop-up is open, or returns it when the pop-up
/// has no use for it
pub(crate) fn apply(state: &mut State, action: Action) -> Result<Vec<Command>, Action> {
    let Some(popup) = state.popup.take() else {
        return Err(action);
    };

    match popup {
        Popup::Prompt { purpose, mut input } => match action {
            Action::Type(key) => {
                input.input(key);
                state.popup = Some(Popup::Prompt { purpose, input });
            }
            Action::ClosePopup => {}
            Action::ConfirmPopup => {
                let text = text_of(&input).trim().to_owned();

                if text.is_empty() {
                    state.popup = Some(Popup::Prompt { purpose, input });
                } else {
                    return Ok(confirm_prompt(state, purpose, input, text));
                }
            }
            other => {
                state.popup = Some(Popup::Prompt { purpose, input });

                return Err(other);
            }
        },
        Popup::Confirm { message, pending } => match action {
            Action::ConfirmPopup => return Ok(run_pending(state, pending)),
            Action::ClosePopup => match pending {
                Pending::CloseVariables(editor) => state.popup = Some(Popup::Variables(editor)),
                Pending::Quit(previous) => state.popup = previous.map(|p| *p),
                _ => {}
            },
            other => {
                state.popup = Some(Popup::Confirm { message, pending });

                return Err(other);
            }
        },
        Popup::Environments {
            collection,
            selected,
        } => return environments(state, collection, selected, action),
        Popup::Variables(editor) => return variables(state, editor, action),
    }

    Ok(Vec::new())
}

fn confirm_prompt(
    state: &mut State,
    purpose: Purpose,
    input: Box<TextArea<'static>>,
    text: String,
) -> Vec<Command> {
    let store = |key: &NodeKey, change| Command::Store {
        collection: key.collection.clone(),
        change,
    };

    match purpose {
        Purpose::SaveBody => {
            // the prompt stays open until the file is written
            state.popup = Some(Popup::Prompt {
                purpose: Purpose::SaveBody,
                input,
            });

            vec![Command::SaveBody(PathBuf::from(text))]
        }
        Purpose::SaveAs(parent) => vec![store(
            &parent,
            Change::CreateRequest {
                parent: parent.path.clone(),
                name: text,
                request: state.request(),
                open: true,
            },
        )],
        Purpose::NewRequest(parent) => vec![store(
            &parent,
            Change::CreateRequest {
                parent: parent.path.clone(),
                name: text,
                request: Request::default(),
                open: false,
            },
        )],
        Purpose::NewFolder(parent) => vec![store(
            &parent,
            Change::CreateFolder {
                parent: parent.path.clone(),
                name: text,
            },
        )],
        Purpose::NewCollection => vec![Command::CreateCollection(text)],
        Purpose::Rename(key) if key.path.is_empty() => {
            vec![store(&key, Change::RenameCollection(text))]
        }
        Purpose::Rename(key) => vec![store(
            &key,
            Change::Rename {
                path: key.path.clone(),
                name: text,
            },
        )],
        Purpose::NewEnvironment(collection) => vec![Command::Store {
            collection,
            change: Change::CreateEnvironment(text),
        }],
        Purpose::RenameEnvironment(collection, id) => vec![Command::Store {
            collection,
            change: Change::RenameEnvironment { id, name: text },
        }],
    }
}

fn run_pending(state: &mut State, pending: Pending) -> Vec<Command> {
    match pending {
        Pending::Open(key) => {
            state.load_request(&key);

            Vec::new()
        }
        Pending::Delete(key) => {
            let change = if key.path.is_empty() {
                Change::DeleteCollection
            } else {
                Change::Delete {
                    path: key.path.clone(),
                }
            };

            vec![Command::Store {
                collection: key.collection,
                change,
            }]
        }
        Pending::DeleteEnvironment(collection, id) => vec![Command::Store {
            collection,
            change: Change::DeleteEnvironment(id),
        }],
        Pending::Quit(_) => {
            state.should_quit = true;

            Vec::new()
        }
        Pending::CloseVariables(_) => Vec::new(),
    }
}

fn environments(
    state: &mut State,
    collection: LocalId,
    mut selected: usize,
    action: Action,
) -> Result<Vec<Command>, Action> {
    let Some(workspace) = state.workspace(&collection) else {
        return Err(action);
    };

    let environments = &workspace.collection.environments;
    let chosen = selected
        .checked_sub(1)
        .and_then(|i| environments.get(i))
        .cloned();

    match action {
        Action::Up => selected = selected.saturating_sub(1),
        Action::Down => selected = (selected + 1).min(environments.len()),
        Action::ClosePopup => return Ok(Vec::new()),
        Action::ConfirmPopup => {
            let id = chosen.map(|e| e.id);

            if let Some(workspace) = state.workspace_mut(&collection) {
                workspace.active_environment.clone_from(&id);
            }

            return Ok(vec![Command::SetActiveEnvironment {
                collection,
                environment: id,
            }]);
        }
        Action::NewEnvironment => {
            state.popup = Some(Popup::prompt(Purpose::NewEnvironment(collection), ""));

            return Ok(Vec::new());
        }
        Action::RenameEnvironment => {
            if let Some(environment) = chosen {
                state.popup = Some(Popup::prompt(
                    Purpose::RenameEnvironment(collection, environment.id),
                    &environment.name,
                ));

                return Ok(Vec::new());
            }
        }
        Action::DeleteEnvironment => {
            if let Some(environment) = chosen {
                state.popup = Some(Popup::confirm(
                    format!(
                        "Delete the environment {} and its variables?",
                        environment.name
                    ),
                    Pending::DeleteEnvironment(collection, environment.id),
                ));

                return Ok(Vec::new());
            }
        }
        Action::EditVariables => {
            let editor = VariablesEditor::new(
                collection,
                chosen,
                &workspace.collection.variables,
                &state.globals,
            );

            state.popup = Some(Popup::Variables(Box::new(editor)));

            return Ok(Vec::new());
        }
        other => {
            state.popup = Some(Popup::Environments {
                collection,
                selected,
            });

            return Err(other);
        }
    }

    state.popup = Some(Popup::Environments {
        collection,
        selected,
    });

    Ok(Vec::new())
}

fn variables(
    state: &mut State,
    mut editor: Box<VariablesEditor>,
    action: Action,
) -> Result<Vec<Command>, Action> {
    match action {
        Action::Type(key) => editor.table_mut().input(key),
        Action::StopEdit => editor.table_mut().commit(),
        Action::NextCell => editor.table_mut().next_cell(),
        Action::NextTab | Action::PreviousTab => {
            let tabs = editor.tabs();

            editor.table_mut().commit();
            editor.tab = step(&tabs, editor.tab, matches!(action, Action::NextTab));
        }
        Action::Up => editor.table_mut().select_previous(),
        Action::Down => editor.table_mut().select_next(),
        Action::AddRow => editor.table_mut().add_row(),
        Action::DeleteRow => editor.table_mut().delete_selected(),
        Action::ToggleRow => editor.table_mut().toggle_selected(),
        Action::Edit => {
            let table = editor.table_mut();

            if table.rows.is_empty() {
                table.add_row();
            } else {
                table.edit(Column::Name);
            }
        }
        Action::Save => return Ok(save_variables(state, editor)),
        Action::ClosePopup => {
            editor.table_mut().commit();

            if editor.is_changed() {
                state.popup = Some(Popup::confirm(
                    "Close without saving the variables?",
                    Pending::CloseVariables(editor),
                ));
            }

            return Ok(Vec::new());
        }
        other => {
            state.popup = Some(Popup::Variables(editor));

            return Err(other);
        }
    }

    state.popup = Some(Popup::Variables(editor));

    Ok(Vec::new())
}

fn save_variables(state: &mut State, mut editor: Box<VariablesEditor>) -> Vec<Command> {
    for table in &mut editor.tables {
        table.commit();
    }

    let mut commands = Vec::new();

    for tab in editor.tabs() {
        if !editor.changed(tab) {
            continue;
        }

        let variables = match editor.variables(tab) {
            Ok(variables) => variables,
            Err((row, message)) => {
                editor.tab = tab;
                editor.table_mut().selected = row;
                state.notice = Some(message);
                state.popup = Some(Popup::Variables(editor));

                return Vec::new();
            }
        };
        let collection = editor.collection.clone();

        commands.push(match tab {
            VarTab::Environment => {
                let Some(mut environment) = editor.environment.clone() else {
                    continue;
                };

                environment.variables = variables;

                Command::Store {
                    collection,
                    change: Change::SaveEnvironment(environment),
                }
            }
            VarTab::Collection => Command::Store {
                collection,
                change: Change::SaveVariables(variables),
            },
            VarTab::Globals => Command::SaveGlobals(variables),
        });
    }

    commands
}
