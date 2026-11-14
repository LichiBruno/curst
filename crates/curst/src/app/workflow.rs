//! running a workflow: each step's request is sent in the background, the
//! values its answer gives are kept as secrets, then the next step goes
//!
//! a run waits in a pop-up of its own, which lists the steps and stops at
//! the first one that fails, leaving every value kept before it in place

use curst_core::http::{HttpError, Response};
use curst_core::model::{Item, LocalId, Variable, Workflow};
use curst_core::secrets::{Approval, Secret, SecretKey};
use curst_core::vars::{Origin, VarName};

use super::popup::{Pending, Popup};
use super::send::{self, Resolved};
use super::{Action, Change, Command, SecretChange, State, Then, Waiting};

/// how far a step got
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StepState {
    Waiting,
    Running,

    /// answered with this status, and its values kept
    Done(u16),
    Failed,
}

/// a workflow being run
#[derive(Debug)]
pub(crate) struct Run {
    pub(crate) collection: LocalId,
    pub(crate) workflow: Workflow,

    /// the step running, or the number of steps once all are done
    pub(crate) step: usize,
    pub(crate) states: Vec<StepState>,

    /// the request of the step on its way
    pub(crate) in_flight: Option<u64>,

    /// why the run stopped, when a step failed
    pub(crate) error: Option<String>,
}

impl Run {
    pub(crate) fn finished(&self) -> bool {
        self.error.is_some() || self.step >= self.workflow.steps.len()
    }
}

/// starts the workflow at `index` of `collection`
pub(crate) fn start(state: &mut State, collection: &LocalId, index: usize) -> Vec<Command> {
    let Some(workspace) = state.workspace(collection) else {
        return Vec::new();
    };

    if !workspace.trusted {
        state.notice = Some(format!(
            "{} is not trusted, so its workflows do not run",
            workspace.collection.name
        ));

        return Vec::new();
    }

    let Some(workflow) = workspace.collection.workflows.get(index).cloned() else {
        return Vec::new();
    };

    if workflow.steps.is_empty() {
        state.notice = Some(format!("{} has no steps yet", workflow.name));

        return Vec::new();
    }

    // a run still showing is left to finish; one whose pop-up is gone,
    // such as one a tab's send took the place of, is replaced
    if state.run.as_ref().is_some_and(|run| !run.finished())
        && matches!(state.popup, Some(Popup::Run))
    {
        state.notice = Some("a workflow is running already".into());

        return Vec::new();
    }

    let mut commands = Vec::new();

    if state.run.take().is_some_and(|run| run.in_flight.is_some()) {
        commands.push(Command::CancelStep);
    }

    state.run = Some(Run {
        collection: collection.clone(),
        states: vec![StepState::Waiting; workflow.steps.len()],
        workflow,
        step: 0,
        in_flight: None,
        error: None,
    });
    state.popup = Some(Popup::Run);
    commands.extend(step(state));

    commands
}

/// stops the run at its current step with `why`
fn fail(state: &mut State, why: String) -> Vec<Command> {
    // a run closed already has nothing to show it on
    let Some(run) = &mut state.run else {
        return Vec::new();
    };

    if let Some(step) = run.states.get_mut(run.step) {
        *step = StepState::Failed;
    }

    run.in_flight = None;
    run.error = Some(why);

    // the run's pop-up shows why, over any question it was waiting on
    state.popup = Some(Popup::Run);

    Vec::new()
}

/// sends the current step's request, or finishes the run after the last
pub(crate) fn step(state: &mut State) -> Vec<Command> {
    let Some(run) = &state.run else {
        return Vec::new();
    };

    if run.finished() {
        return Vec::new();
    }

    // back from a question, such as the passphrase, the run shows again
    if state.popup.is_none() {
        state.popup = Some(Popup::Run);
    }

    let number = run.step + 1;
    let step = run.workflow.steps[run.step].clone();
    let collection = run.collection.clone();
    let Some(workspace) = state.workspace(&collection).filter(|w| w.trusted) else {
        return fail(state, "the collection is gone".into());
    };
    let Some(Item::Request(saved)) = workspace.collection.item(&step.request) else {
        return fail(
            state,
            format!("step {number}: its request is no longer in the collection"),
        );
    };
    let request = workspace
        .collection
        .with_inherited_auth(&step.request, &saved.request);

    // a step that cannot go for want of what it is there to get, such as
    // a login request inheriting Bearer {{token}}, says how to fix it
    let text = curst_core::curl::emit(&request);
    let own = step
        .keep
        .iter()
        .find(|c| text.contains(&format!("{{{{{}}}}}", c.variable)))
        .map(|capture| {
            format!(
                "step {number}: {} uses {{{{{}}}}}, which this step sets: give it auth none or its own",
                saved.name, capture.variable
            )
        });

    match send::resolve_in(state, &request, Some(workspace)) {
        Resolved::Ready { request, secrets } => {
            state.next_id += 1;

            let id = state.next_id;

            if let Some(run) = &mut state.run {
                run.in_flight = Some(id);
                run.states[run.step] = StepState::Running;
            }

            vec![Command::SendStep {
                id,
                request,
                secrets,
            }]
        }
        Resolved::Blocked(notice) | Resolved::Empty(_, notice) => {
            let why = own.unwrap_or_else(|| format!("step {number}: {notice}"));

            fail(state, why)
        }
        Resolved::Load(keys) => {
            state.next_id += 1;
            state.waiting = Some(Waiting {
                id: state.next_id,
                then: Then::Workflow,
            });

            vec![Command::LoadSecrets {
                id: state.next_id,
                keys,
            }]
        }
    }
}

/// the answer to the current step: its values are kept, then the next
/// step goes
pub(crate) fn answered(
    state: &mut State,
    id: u64,
    result: Result<Response, HttpError>,
) -> Vec<Command> {
    let Some(run) = &mut state.run else {
        return Vec::new();
    };

    if run.in_flight != Some(id) {
        return Vec::new();
    }

    run.in_flight = None;

    let number = run.step + 1;
    let keep = run.workflow.steps[run.step].keep.clone();
    let response = match result {
        Ok(response) => response,
        Err(err) => return fail(state, format!("step {number}: {err}")),
    };

    if !(200..300).contains(&response.status) {
        return fail(
            state,
            format!("step {number}: the answer was {}", response.status_line),
        );
    }

    let mut values = Vec::new();

    for capture in &keep {
        match curst_core::workflow::extract(&response, &capture.from) {
            Ok(value) => values.push((capture.variable.clone(), value)),
            Err(why) => return fail(state, format!("step {number}: {why}")),
        }
    }

    if let Some(run) = &mut state.run {
        run.states[run.step] = StepState::Done(response.status);
    }

    keep_values(state, values)
}

/// a step's request needs approval before it carries its secrets
pub(crate) fn needs_approval(state: &mut State, id: u64, approval: Approval) {
    let Some(run) = &mut state.run else {
        return;
    };

    if run.in_flight != Some(id) {
        return;
    }

    run.in_flight = None;
    state.waiting = Some(Waiting {
        id,
        then: Then::Workflow,
    });
    state.popup = Some(Popup::Approve(approval));
}

/// stores the values a step kept, each as a secret where the variable is
/// defined nearest, else in the active environment, else the collection;
/// the run goes on once they are stored
fn keep_values(state: &mut State, values: Vec<(VarName, String)>) -> Vec<Command> {
    if values.is_empty() {
        return next(state);
    }

    let Some(run) = &state.run else {
        return Vec::new();
    };

    let collection = run.collection.clone();
    let Some(workspace) = state.workspace(&collection) else {
        return fail(state, "the collection is gone".into());
    };

    let mut environment = workspace
        .active_environment
        .as_ref()
        .and_then(|id| workspace.collection.environment(id))
        .cloned();
    let mut variables = workspace.collection.variables.clone();
    let (mut environment_changed, mut variables_changed) = (false, false);
    let mut changes = Vec::new();

    for (name, value) in values {
        let defined = |list: &[Variable]| list.iter().find(|v| v.name == name).cloned();
        let in_environment = environment.as_ref().and_then(|e| defined(&e.variables));
        let in_collection = defined(&variables);
        let origin = match (&in_environment, &in_collection, &mut environment) {
            (Some(_), _, _) => Origin::Environment,
            (None, Some(_), _) => Origin::Collection,
            (None, None, Some(environment)) => {
                environment.variables.push(Variable::secret(name.clone()));
                environment_changed = true;

                Origin::Environment
            }
            (None, None, None) => {
                variables.push(Variable::secret(name.clone()));
                variables_changed = true;

                Origin::Collection
            }
        };

        // a plain variable's value is in a file, where a token never goes
        if in_environment.or(in_collection).is_some_and(|v| !v.secret) {
            return fail(
                state,
                format!(
                    "{{{{{name}}}}} is a plain variable: make it a secret to keep values in it"
                ),
            );
        }

        let key = SecretKey::for_variable(
            origin,
            Some(&collection),
            environment.as_ref().map(|e| &e.id),
            name,
        );

        if let Some(key) = key {
            changes.push(SecretChange::Set(key, Secret::new(value)));
        }
    }

    let mut files = Vec::new();

    // the variables are known at once, so the next step resolves them
    // before the files are read back
    if let Some(workspace) = state.workspace_mut(&collection) {
        if environment_changed && let Some(environment) = &environment {
            if let Some(slot) = workspace
                .collection
                .environments
                .iter_mut()
                .find(|e| e.id == environment.id)
            {
                slot.clone_from(environment);
            }

            files.push(Command::Store {
                collection: collection.clone(),
                change: Change::SaveEnvironment(environment.clone()),
            });
        }

        if variables_changed {
            workspace.collection.variables.clone_from(&variables);
            files.push(Command::Store {
                collection: collection.clone(),
                change: Change::SaveVariables(variables),
            });
        }
    }

    store_values(state, changes, files)
}

/// stores `changes` in the secret store, then writes `files` and goes on
pub(crate) fn store_values(
    state: &mut State,
    changes: Vec<SecretChange>,
    files: Vec<Command>,
) -> Vec<Command> {
    state.next_id += 1;
    state.waiting = Some(Waiting {
        id: state.next_id,
        then: Then::Kept {
            changes: changes.clone(),
            files,
        },
    });

    vec![Command::ChangeSecrets {
        id: state.next_id,
        changes,
    }]
}

/// the values are stored: the next step goes
pub(crate) fn kept(state: &mut State, files: Vec<Command>) -> Vec<Command> {
    let mut commands = files;

    commands.extend(next(state));

    commands
}

/// the values could not be stored: the run stops there
pub(crate) fn not_kept(state: &mut State, why: &str) -> Vec<Command> {
    fail(state, format!("the values were not kept: {why}"))
}

/// the secret store question was left unanswered, or approval refused
pub(crate) fn given_up(state: &mut State) -> Vec<Command> {
    fail(state, "stopped before the step was sent".into())
}

fn next(state: &mut State) -> Vec<Command> {
    if let Some(run) = &mut state.run {
        run.step += 1;
    }

    step(state)
}

/// Esc in the run's pop-up: a step on its way is canceled, the run ends
pub(crate) fn close(state: &mut State) -> Vec<Command> {
    let Some(run) = state.run.take() else {
        return Vec::new();
    };

    // values being stored are left to be stored, and their files written
    if matches!(
        state.waiting.as_ref().map(|w| &w.then),
        Some(Then::Workflow)
    ) {
        state.waiting = None;
    }

    if run.finished() && run.error.is_none() {
        state.notice = Some(format!("{} is done", run.workflow.name));
    }

    if run.in_flight.is_some() {
        vec![Command::CancelStep]
    } else {
        Vec::new()
    }
}

/// opens the workflows of `collection`
pub(crate) fn list(state: &mut State, collection: &LocalId) {
    state.popup = Some(Popup::Workflows {
        collection: collection.clone(),
        selected: 0,
    });
}

/// a key in the list of workflows: Enter runs one, or makes a new one on
/// the last row; `e` edits, `d` deletes
pub(crate) fn list_key(
    state: &mut State,
    collection: LocalId,
    selected: usize,
    action: Action,
) -> Result<Vec<Command>, Action> {
    let count = state
        .workspace(&collection)
        .map_or(0, |w| w.collection.workflows.len());
    let keep = |state: &mut State, selected: usize| {
        state.popup = Some(Popup::Workflows {
            collection: collection.clone(),
            selected,
        });
    };

    match action {
        Action::Up => keep(state, selected.saturating_sub(1)),
        Action::Down => keep(state, (selected + 1).min(count)),
        Action::Pick(index) if index < count => return Ok(start(state, &collection, index)),
        Action::Pick(_) => super::workflow_editor::open(state, &collection, None),
        Action::ConfirmPopup if selected >= count => {
            super::workflow_editor::open(state, &collection, None);
        }
        Action::ConfirmPopup => return Ok(start(state, &collection, selected)),
        Action::Edit if selected < count => {
            super::workflow_editor::open(state, &collection, Some(selected));
        }
        Action::DeleteRow if selected < count => {
            let name = state.workspace(&collection).map_or_else(String::new, |w| {
                w.collection.workflows[selected].name.clone()
            });

            state.popup = Some(Popup::confirm(
                format!("Delete the workflow {name}?"),
                Pending::DeleteWorkflow(collection, selected),
            ));
        }
        Action::ClosePopup => {}
        other => {
            keep(state, selected);

            return Err(other);
        }
    }

    Ok(Vec::new())
}

/// deletes the workflow at `index` of `collection`
pub(crate) fn delete(state: &State, collection: &LocalId, index: usize) -> Vec<Command> {
    let Some(workspace) = state.workspace(collection) else {
        return Vec::new();
    };

    let mut workflows = workspace.collection.workflows.clone();

    if index < workflows.len() {
        workflows.remove(index);
    }

    vec![Command::Store {
        collection: collection.clone(),
        change: Change::SaveWorkflows(workflows),
    }]
}

/// a key in the run's pop-up: Esc stops and closes it, Enter closes it
/// once the run is over
pub(crate) fn run_key(state: &mut State, action: Action) -> Result<Vec<Command>, Action> {
    match action {
        Action::ClosePopup => Ok(close(state)),
        Action::ConfirmPopup if state.run.as_ref().is_none_or(Run::finished) => Ok(close(state)),
        Action::ConfirmPopup => {
            state.popup = Some(Popup::Run);

            Ok(Vec::new())
        }
        other => {
            state.popup = Some(Popup::Run);

            Err(other)
        }
    }
}
