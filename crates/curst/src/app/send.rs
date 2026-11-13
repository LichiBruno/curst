//! sending: resolving variables, fetching the secret values a request
//! needs, and waiting for an approval before a secret goes to a new origin
//!
//! the runtime has the last word on approval: `update` only says which
//! secrets a request carries, and the runtime checks them against the index
//! just before the transfer starts

use std::collections::{BTreeSet, HashMap};

use curst_core::model::{Body, EnvId, LocalId, Request};
use curst_core::secrets::{Approval, Secret, SecretKey};
use curst_core::store::Backend;
use curst_core::vars::{Origin, Resolution, Scopes, SecretValues, VarName, resolve_request};

use super::popup::{Popup, VariablesEditor};
use super::{
    Command, Focus, Layer, Locked, Outcome, SecretChange, State, Then, Waiting, Workspace,
};

const OPENING: &str = "opening secrets…";

/// secret values fetched this session, read through the scope they belong to
struct Values<'a> {
    fetched: &'a HashMap<SecretKey, Option<Secret>>,
    collection: Option<&'a LocalId>,
    environment: Option<&'a EnvId>,
}

impl SecretValues for Values<'_> {
    fn secret(&self, origin: Origin, name: &VarName) -> Option<&str> {
        let key = SecretKey::for_variable(origin, self.collection, self.environment, name.clone())?;

        self.fetched
            .get(&key)
            .and_then(Option::as_ref)
            .map(Secret::expose)
    }
}

pub(super) fn send(state: &mut State) -> Vec<Command> {
    state.stop_editing();

    let request = state.request();

    if request.url.is_empty() {
        state.notice = Some("type a URL first".into());
        state.focus = Focus::Url;

        return Vec::new();
    }

    if let Some(open) = &state.open
        && state
            .workspace(&open.key.collection)
            .is_some_and(|w| !w.trusted)
    {
        state.notice = Some(format!(
            "not sent: {} is in a collection you have not trusted; select it and press {}",
            open.name,
            state.key(Layer::Sidebar, "trust")
        ));

        return Vec::new();
    }

    send_request(state, request)
}

/// resolves and sends `request`, or asks for what is missing first
pub(super) fn send_request(state: &mut State, request: Request) -> Vec<Command> {
    // an untrusted collection lends no variables, even to a new request
    let workspace = state.current_workspace().filter(|w| w.trusted);
    let collection = workspace.map(|w| &w.id);
    let environment = workspace.and_then(|w| {
        w.active_environment
            .as_ref()
            .filter(|id| w.collection.environment(id).is_some())
    });
    let values = Values {
        fetched: &state.secret_values,
        collection,
        environment,
    };
    let scopes = Scopes {
        environment: workspace.map_or(&[], Workspace::environment_variables),
        collection: workspace.map_or(&[], |w| w.collection.variables.as_slice()),
        global: &state.globals,
        secrets: Some(&values),
    };
    let (resolved, resolution) = resolve_request(&request, &scopes);
    let key = |(origin, name): &(Origin, VarName)| {
        SecretKey::for_variable(*origin, collection, environment, name.clone())
    };

    if !resolution.missing.is_empty() || !resolution.cyclic.is_empty() {
        state.notice = Some(not_sent(workspace, &resolution));

        return Vec::new();
    }

    if !resolution.unavailable.is_empty() {
        let keys: Vec<SecretKey> = resolution.unavailable.iter().filter_map(key).collect();
        let unknown: Vec<SecretKey> = keys
            .into_iter()
            .filter(|k| !state.secret_values.contains_key(k))
            .collect();

        // the values are known, and some are empty: they are asked for, and
        // the request goes once they are stored
        if unknown.is_empty() {
            let empty: Vec<(SecretKey, String)> = resolution
                .unavailable
                .iter()
                .filter_map(key)
                .map(|k| {
                    let label = super::fill::label(workspace, &k);

                    (k, label)
                })
                .collect();

            state.notice = Some(not_sent(workspace, &resolution));
            super::fill::open(state, empty, Some(request));

            return Vec::new();
        }

        state.next_id += 1;
        state.waiting = Some(Waiting {
            id: state.next_id,
            then: Then::Send(request),
        });
        state.notice = Some(OPENING.into());

        return vec![Command::LoadSecrets {
            id: state.next_id,
            keys: unknown,
        }];
    }

    let secrets: Vec<SecretKey> = resolution.secrets.iter().filter_map(key).collect();
    let mut commands = Vec::new();

    if state.in_flight.is_some() {
        commands.push(Command::Cancel);
    }

    state.next_id += 1;
    state.in_flight = Some(state.next_id);
    state.waiting = None;
    state.last_sent = Some(request);

    // the request is on its way, so the wait for secrets is over
    if state.notice.as_deref() == Some(OPENING) {
        state.notice = None;
    }

    commands.push(Command::Send {
        id: state.next_id,
        request: resolved,
        secrets,
    });

    commands
}

/// why a request was not sent, naming every variable that blocks it
fn not_sent(workspace: Option<&Workspace>, resolution: &Resolution) -> String {
    let braced = |names: &mut dyn Iterator<Item = &VarName>| {
        names
            .map(|n| format!("{{{{{n}}}}}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut reasons = Vec::new();

    if !resolution.missing.is_empty() {
        let place = workspace
            .and_then(Workspace::environment_name)
            .map_or_else(String::new, |env| format!(" in {env}"));

        reasons.push(format!(
            "{} not defined{place}",
            braced(&mut resolution.missing.iter())
        ));
    }

    if !resolution.unavailable.is_empty() {
        let names: BTreeSet<&VarName> = resolution.unavailable.iter().map(|(_, n)| n).collect();

        reasons.push(format!(
            "{} secret with no value yet; set it in the variable editor",
            braced(&mut names.into_iter())
        ));
    }

    if !resolution.cyclic.is_empty() {
        reasons.push(format!(
            "{} refer to themselves",
            braced(&mut resolution.cyclic.iter())
        ));
    }

    format!("not sent: {}", reasons.join("; "))
}

/// starts again the work that was waiting under `id`, if it still is
fn retry(state: &mut State, id: Option<u64>) -> Vec<Command> {
    match state.waiting.take() {
        Some(waiting) if id.is_none_or(|id| id == waiting.id) => match waiting.then {
            Then::Send(request) => send_request(state, request),
            Then::Save {
                changes,
                files,
                send,
            } => save_secrets(state, changes, files, send),
            Then::Migrate { to, passphrase } => start_migration(state, to, passphrase),
            Then::Reveal { editor, key } => reveal(state, editor, key),
        },
        other => {
            state.waiting = other;

            Vec::new()
        }
    }
}

/// changes the secret store, then, once that worked, writes `files`; a
/// variable file never names a secret whose value could not be stored
pub(super) fn change_secrets(
    state: &mut State,
    changes: Vec<SecretChange>,
    files: Vec<Command>,
) -> Vec<Command> {
    save_secrets(state, changes, files, None)
}

/// stores secret values, then sends `request` when one waits for them
pub(super) fn change_secrets_then_send(
    state: &mut State,
    changes: Vec<SecretChange>,
    request: Option<Request>,
) -> Vec<Command> {
    save_secrets(state, changes, Vec::new(), request)
}

fn save_secrets(
    state: &mut State,
    changes: Vec<SecretChange>,
    files: Vec<Command>,
    send: Option<Request>,
) -> Vec<Command> {
    if changes.is_empty() {
        return files;
    }

    state.next_id += 1;
    state.waiting = Some(Waiting {
        id: state.next_id,
        then: Then::Save {
            changes: changes.clone(),
            files,
            send,
        },
    });

    vec![Command::ChangeSecrets {
        id: state.next_id,
        changes,
    }]
}

pub(super) fn changed(state: &mut State, id: u64, result: Result<(), String>) -> Vec<Command> {
    let Some(waiting) = state.waiting.take_if(|w| w.id == id) else {
        return Vec::new();
    };

    // values fetched before may have changed, so they are fetched again
    state.secret_values.clear();

    match (result, waiting.then) {
        (Ok(()), Then::Save { files, send, .. }) => {
            if state.notice.is_none() {
                state.notice = Some("secrets saved".into());
            }

            let mut commands = files;

            if let Some(request) = send {
                commands.extend(send_request(state, request));
            }

            commands
        }
        (Err(message), _) => {
            state.notice = Some(format!("nothing was saved: {message}"));

            Vec::new()
        }
        (Ok(()), Then::Send(_) | Then::Migrate { .. } | Then::Reveal { .. }) => Vec::new(),
    }
}

pub(super) fn loaded(
    state: &mut State,
    id: u64,
    values: Vec<(SecretKey, Option<Secret>)>,
) -> Vec<Command> {
    state.secret_values.extend(values);

    retry(state, Some(id))
}

pub(super) fn locked(state: &mut State, id: Option<u64>, locked: Locked) {
    let waiting = state.waiting.as_ref().map(|w| w.id);

    if id.is_some() && id != waiting {
        return;
    }

    state.popup = match locked {
        Locked::Backend => Some(Popup::ChooseBackend),
        Locked::Passphrase { new, error } => Some(Popup::passphrase(new, error)),
        Locked::Failed(message) => {
            state.notice = Some(message);
            give_back(state);

            state.popup.take()
        }
    };
}

/// the store opened: every value fetched before belongs to whatever was
/// open then, so they are fetched again
pub(super) fn unlocked(state: &mut State) -> Vec<Command> {
    state.secret_values.clear();
    state.notice = Some("secrets unlocked".into());

    retry(state, None)
}

pub(super) fn backend_chosen(state: &mut State, result: Result<Backend, String>) -> Vec<Command> {
    state.secret_values.clear();

    match result {
        Ok(backend) => {
            state.backend = Some(backend);

            retry(state, None)
        }
        Err(message) => {
            state.notice = Some(message);
            state.popup = Some(Popup::ChooseBackend);

            Vec::new()
        }
    }
}

/// the runtime refused to send until the user approves `approval`
pub(super) fn needs_approval(state: &mut State, id: u64, approval: Approval) {
    // the question is about that tab's request, so it is shown there
    super::tabs::focus_sender(state, id);

    if state.in_flight != Some(id) {
        return;
    }

    state.in_flight = None;

    if let Some(request) = state.last_sent.clone() {
        state.waiting = Some(Waiting {
            id,
            then: Then::Send(request),
        });
        state.popup = Some(Popup::Approve(approval));
    }
}

pub(super) fn approved(state: &mut State, id: u64, result: Result<(), String>) -> Vec<Command> {
    match result {
        Ok(()) => retry(state, Some(id)),
        Err(message) => {
            state.waiting = None;
            state.notice = Some(message);

            Vec::new()
        }
    }
}

/// sends the request a stopped redirect pointed to, through the same
/// approval as any other
pub(super) fn follow(state: &mut State) -> Vec<Command> {
    let Some(Outcome::Response(view)) = &state.outcome else {
        return Vec::new();
    };

    let (Some(stopped), Some(last)) = (&view.response.stopped, &state.last_sent) else {
        state.notice = Some("there is no redirect to follow".into());

        return Vec::new();
    };

    let request = Request {
        method: stopped.method,
        url: stopped.url.clone(),
        params: Vec::new(),
        headers: last.headers.clone(),
        body: if stopped.keeps_body {
            last.body.clone()
        } else {
            Body::None
        },
        auth: last.auth.clone(),
    };

    send_request(state, request)
}

/// gives a secret's value back to the variable editor, as a plain value;
/// the value is fetched first when this session has not seen it
pub(super) fn reveal(
    state: &mut State,
    mut editor: Box<VariablesEditor>,
    key: SecretKey,
) -> Vec<Command> {
    if let Some(value) = state.secret_values.get(&key) {
        let value = value
            .as_ref()
            .map_or_else(String::new, |v| v.expose().to_owned());

        editor.table_mut().make_plain(value);
        state.popup = Some(Popup::Variables(editor));
        state.notice = Some(format!(
            "it is a plain variable again; {} writes its value to the file and deletes the secret",
            state.key(Layer::Variables, "save")
        ));

        return Vec::new();
    }

    state.next_id += 1;
    state.notice = Some(OPENING.into());
    state.waiting = Some(Waiting {
        id: state.next_id,
        then: Then::Reveal {
            editor,
            key: key.clone(),
        },
    });

    vec![Command::LoadSecrets {
        id: state.next_id,
        keys: vec![key],
    }]
}

/// drops the work that was waiting; a variable editor waiting for a value
/// comes back as it was
pub(super) fn give_back(state: &mut State) {
    if let Some(Waiting {
        then: Then::Reveal { editor, .. },
        ..
    }) = state.waiting.take()
    {
        state.popup = Some(Popup::Variables(editor));
    }
}

/// moves every secret to `to`, waiting like a send if the store has to be
/// opened first
pub(super) fn start_migration(
    state: &mut State,
    to: Backend,
    passphrase: Option<Secret>,
) -> Vec<Command> {
    state.next_id += 1;
    state.waiting = Some(Waiting {
        id: state.next_id,
        then: Then::Migrate {
            to,
            passphrase: passphrase.clone(),
        },
    });
    state.notice = Some("moving secrets…".into());

    vec![Command::Migrate {
        id: state.next_id,
        to,
        passphrase,
    }]
}

pub(super) fn migrated(state: &mut State, id: u64, result: Result<(Backend, usize), String>) {
    if state.waiting.take_if(|w| w.id == id).is_none() {
        return;
    }

    state.secret_values.clear();
    state.notice = Some(match result {
        Ok((backend, count)) => {
            state.backend = Some(backend);

            let place = match backend {
                Backend::Keychain => "the keychain",
                Backend::EncryptedFile => "the encrypted file",
            };

            match count {
                1 => format!("1 secret moved to {place}"),
                n => format!("{n} secrets moved to {place}"),
            }
        }
        Err(message) => message,
    });
}

/// what the passphrase prompt does on Enter; `migrate` when it is the
/// passphrase of a file secrets are about to move into
pub(super) fn passphrase(
    state: &mut State,
    (new, migrate): (bool, bool),
    first: Option<Secret>,
    typed: Secret,
) -> Vec<Command> {
    let ask_again = |error: &str| Popup::Passphrase {
        new,
        migrate,
        first: None,
        input: Box::new(super::editor::masked("passphrase")),
        error: Some(error.into()),
    };
    let done = |state: &mut State, typed: Secret| {
        if migrate {
            start_migration(state, Backend::EncryptedFile, Some(typed))
        } else {
            vec![Command::Unlock(typed)]
        }
    };

    if typed.expose().is_empty() {
        state.popup = Some(ask_again("the passphrase cannot be empty"));

        return Vec::new();
    }

    if !new {
        return done(state, typed);
    }

    match first {
        None => {
            state.popup = Some(Popup::Passphrase {
                new,
                migrate,
                first: Some(typed),
                input: Box::new(super::editor::masked("again, to be sure")),
                error: None,
            });

            Vec::new()
        }
        Some(first) if first == typed => done(state, typed),
        Some(_) => {
            state.popup = Some(ask_again("the two did not match, start again"));

            Vec::new()
        }
    }
}

/// what the settings `m` key does: choose a backend when none is chosen,
/// otherwise move the secrets to the other one
pub(super) fn migrate(state: &mut State) -> Vec<Command> {
    match state.backend {
        None => {
            state.popup = Some(Popup::ChooseBackend);

            Vec::new()
        }
        Some(Backend::EncryptedFile) => start_migration(state, Backend::Keychain, None),
        Some(Backend::Keychain) => {
            state.popup = Some(Popup::Passphrase {
                new: true,
                migrate: true,
                first: None,
                input: Box::new(super::editor::masked("passphrase")),
                error: None,
            });

            Vec::new()
        }
    }
}
