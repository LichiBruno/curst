//! importing an `OpenAPI` spec: where it is, a preview of what it becomes,
//! then the import, which asks for the secrets it names

use curst_core::curl;
use curst_core::model::{Auth, KeyValue, LocalId, Request, Source, Variable};
use curst_core::openapi::Converted;
use curst_core::secrets::SecretKey;

use super::popup::{Popup, Purpose};
use super::send::{Resolved, resolve_in};
use super::{Action, Command, SecretChange, State, Then, Waiting};

/// a spec read and converted, waiting for the user to look it over
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Preview {
    pub(crate) converted: Converted,

    /// where it came from, kept with the collection to fetch it again
    pub(crate) source: Source,

    /// the first row shown, once its lines are wrapped to the pop-up
    pub(crate) scroll: usize,

    /// the furthest it scrolls, as it was last drawn: the wrapped rows
    /// past what fits
    pub(crate) last_scroll: std::cell::Cell<usize>,
}

impl Preview {
    pub(crate) fn new(converted: Converted, source: Source) -> Self {
        Self {
            converted,
            source,
            scroll: 0,
            last_scroll: std::cell::Cell::new(0),
        }
    }

    /// the lines the preview shows, the summary first
    pub(crate) fn lines(&self) -> Vec<String> {
        let c = &self.converted;
        let mut lines = vec![
            format!(
                "{} in {}, from {}",
                count(c.requests, "request", "requests"),
                count(c.folders, "folder", "folders"),
                self.source.location
            ),
            match c.bundle.environments.len() {
                0 => "no server is named: set base_url to where the API is".to_owned(),
                1 => "1 server, as an environment holding the variables".to_owned(),
                n => format!("{n} servers, an environment each, each holding the variables"),
            },
        ];

        if !c.secrets.is_empty() {
            lines.push(format!(
                "secrets to fill in after: {}",
                c.secrets.join(", ")
            ));
        }

        if !c.credentials.is_empty() {
            lines.push(String::new());
            lines.push(match c.credentials.len() {
                1 => "1 example value looked like a credential and was left out:".to_owned(),
                n => format!("{n} example values looked like credentials and were left out:"),
            });
            lines.extend(c.credentials.iter().map(|w| format!("  {w}")));
        }

        if !c.bundle.warnings.is_empty() {
            lines.push(String::new());
            lines.push("worth knowing:".to_owned());
            lines.extend(c.bundle.warnings.iter().map(|w| format!("  {w}")));
        }

        lines
    }
}

/// `n` and the word for it, one or many
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// what reading a location found
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Found {
    /// a spec, converted
    Preview(Box<Preview>),

    /// a folder with several specs, to choose from
    Choose { folder: String, specs: Vec<String> },
}

/// whether a location is a URL rather than a file or folder
pub(crate) fn is_url(location: &str) -> bool {
    let lower = location.trim().to_ascii_lowercase();

    lower.starts_with("http://") || lower.starts_with("https://")
}

/// reads a spec from where the user said: a URL is fetched in the
/// background, a file or folder read at once
pub(crate) fn start(state: &mut State, location: &str) -> Vec<Command> {
    let location = location.trim().to_owned();

    // a curl command gives the URL and the headers, such as a token
    if super::request::is_curl(&location) {
        let (stored, now, changes, variables) = match from_curl(state, &location) {
            Ok(parts) => parts,
            Err(why) => {
                state.notice = Some(format!("not imported: {why}"));

                return Vec::new();
            }
        };
        let job = Job {
            target: Target::Import,
            source: stored,
        };

        // fetched with the values as given, kept with placeholders; the
        // fetch waits for the secrets to be stored, so a question about
        // where to keep them is never covered by the preview
        let fetch = fetch(
            state,
            job,
            Pending::Fetch {
                resolved: now,
                secrets: Vec::new(),
            },
        );

        return keep_secrets(state, changes, variables, fetch);
    }

    run(
        state,
        Job {
            target: Target::Import,
            source: Source {
                location,
                ..Source::default()
            },
        },
    )
}

/// a fetched spec, unless the user has moved on from it
pub(crate) fn fetched(state: &mut State, id: u64, result: Result<Box<Preview>, String>) {
    if state.spec_fetch != Some(id) {
        return;
    }

    state.spec_fetch = None;
    state.notice = None;
    read(state, result.map(Found::Preview));
}

/// a key in the list of a folder's specs: Enter or a click reads one
pub(crate) fn choice_key(
    state: &mut State,
    folder: String,
    specs: Vec<String>,
    mut selected: usize,
    action: Action,
) -> Result<Vec<Command>, Action> {
    let last = specs.len().saturating_sub(1);

    match action {
        Action::Up => selected = selected.saturating_sub(1),
        Action::Down => selected = (selected + 1).min(last),
        Action::ConfirmPopup | Action::Pick(_) => {
            let index = match action {
                Action::Pick(index) => index,
                _ => selected,
            };

            return Ok(specs
                .get(index)
                .map(|picked| Command::ReadSpec {
                    location: folder,
                    picked: Some(picked.clone()),
                })
                .into_iter()
                .collect());
        }
        Action::ClosePopup => return Ok(Vec::new()),
        other => {
            state.popup = Some(Popup::SpecChoice {
                folder,
                specs,
                selected,
            });

            return Err(other);
        }
    }

    state.popup = Some(Popup::SpecChoice {
        folder,
        specs,
        selected,
    });

    Ok(Vec::new())
}

/// asks where the spec is
pub(crate) fn ask(state: &mut State) {
    state.popup = Some(Popup::prompt(Purpose::ImportSpec, ""));
}

/// a key in the preview: Enter imports, Esc leaves it, the arrows scroll
pub(crate) fn preview_key(
    state: &mut State,
    mut preview: Box<Preview>,
    action: Action,
) -> Result<Vec<Command>, Action> {
    let last = preview.last_scroll.get();

    match action {
        Action::ConfirmPopup => return Ok(vec![Command::ImportSpec(preview)]),
        Action::ClosePopup => {
            state.notice = Some("nothing was imported".into());

            return Ok(Vec::new());
        }
        Action::Up => preview.scroll = preview.scroll.saturating_sub(1),
        Action::Down => preview.scroll = (preview.scroll + 1).min(last),
        other => {
            state.popup = Some(Popup::SpecPreview(preview));

            return Err(other);
        }
    }

    state.popup = Some(Popup::SpecPreview(preview));

    Ok(Vec::new())
}

/// what reading the spec gave: the preview, the specs of a folder to
/// choose from, or why there is none
pub(crate) fn read(state: &mut State, result: Result<Found, String>) {
    match result {
        Ok(Found::Preview(preview)) => state.popup = Some(Popup::SpecPreview(preview)),
        Ok(Found::Choose { folder, specs }) => {
            state.popup = Some(Popup::SpecChoice {
                folder,
                specs,
                selected: 0,
            });
        }
        Err(message) => state.notice = Some(format!("not imported: {message}")),
    }
}

/// what a spec is fetched for
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    Import,
    Refresh(LocalId),
}

/// a spec to fetch: from where, and why; its URL and headers may hold
/// `{{variables}}`, resolved just before it is fetched
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Job {
    pub(crate) target: Target,
    pub(crate) source: Source,
}

/// fetches the spec of `job`: its variables resolved in the collection it
/// is for, or the globals for an import; secrets are fetched from the store
/// first, and asked for when they have no value
pub(crate) fn run(state: &mut State, job: Job) -> Vec<Command> {
    let collection = match &job.target {
        Target::Refresh(collection) => Some(collection.clone()),
        Target::Import => None,
    };

    // a file or a folder is read as it is
    if !is_url(&job.source.location) {
        return match collection {
            Some(collection) => fetch(
                state,
                job.clone(),
                collection_load(collection, job.source, Vec::new()),
            ),
            None => vec![Command::ReadSpec {
                location: job.source.location,
                picked: job.source.picked,
            }],
        };
    }

    let request = Request {
        url: job.source.location.clone(),
        headers: job.source.headers.clone(),
        ..Request::default()
    };

    // an untrusted collection lends no variables
    let workspace = collection
        .as_ref()
        .and_then(|c| state.workspace(c))
        .filter(|w| w.trusted);

    match resolve_in(state, &request, workspace) {
        Resolved::Ready { request, secrets } => {
            let resolved = Source {
                location: request.url,
                headers: request.headers,
                picked: job.source.picked.clone(),
            };

            match job.target.clone() {
                Target::Import => fetch(state, job.clone(), Pending::Fetch { resolved, secrets }),
                Target::Refresh(collection) => {
                    fetch(state, job, collection_load(collection, resolved, secrets))
                }
            }
        }
        Resolved::Blocked(notice) => {
            state.notice = Some(notice.replacen("not sent", "the spec was not fetched", 1));

            Vec::new()
        }
        Resolved::Empty(empty, notice) => {
            state.notice = Some(notice.replacen("not sent", "the spec was not fetched", 1));
            super::fill::open(state, empty, None);

            Vec::new()
        }
        Resolved::Load(keys) => {
            state.next_id += 1;
            state.waiting = Some(Waiting {
                id: state.next_id,
                then: Then::Spec(job),
            });
            state.notice = Some("opening secrets…".into());

            vec![Command::LoadSecrets {
                id: state.next_id,
                keys,
            }]
        }
    }
}

/// a command waiting for its id
enum Pending {
    Fetch {
        resolved: Source,
        secrets: Vec<SecretKey>,
    },
    Load {
        collection: LocalId,
        resolved: Source,
        secrets: Vec<SecretKey>,
    },
}

fn collection_load(collection: LocalId, resolved: Source, secrets: Vec<SecretKey>) -> Pending {
    Pending::Load {
        collection,
        resolved,
        secrets,
    }
}

/// gives the fetch an id, remembered with its job in case the host must be
/// approved first
fn fetch(state: &mut State, job: Job, pending: Pending) -> Vec<Command> {
    state.next_id += 1;

    let id = state.next_id;

    state.spec_job = Some((id, job.clone()));

    match pending {
        Pending::Fetch { resolved, secrets } => {
            state.spec_fetch = Some(id);
            state.notice = Some(format!("looking for the spec at {}…", job.source.location));

            vec![Command::FetchSpec {
                id,
                url: resolved.location,
                headers: resolved.headers,
                secrets,
                source: job.source,
            }]
        }
        Pending::Load {
            collection,
            resolved,
            secrets,
        } => {
            state.refresh_fetch = Some(id);
            state.notice = Some(format!(
                "fetching the spec again from {}…",
                job.source.location
            ));

            vec![Command::LoadSpec {
                id,
                collection,
                source: resolved,
                secrets,
            }]
        }
    }
}

/// a spec fetch carrying secrets to a host they were never sent to: the
/// same question as for a request, and the fetch goes once it is answered
pub(crate) fn needs_approval(state: &mut State, id: u64, approval: curst_core::secrets::Approval) {
    let Some((job_id, job)) = state.spec_job.take() else {
        return;
    };

    if job_id != id {
        state.spec_job = Some((job_id, job));

        return;
    }

    state.spec_fetch = None;
    state.refresh_fetch = None;
    state.notice = None;
    state.waiting = Some(Waiting {
        id,
        then: Then::Spec(job),
    });
    state.popup = Some(Popup::Approve(approval));
}

/// a source given as a curl command: the source to keep, with `{{name}}`
/// where the command had a credential, the same source with the values to
/// fetch it now, and the secrets to keep, as global ones
pub(crate) fn from_curl(
    state: &State,
    text: &str,
) -> Result<(Source, Source, Vec<SecretChange>, Vec<Variable>), String> {
    let literal = curl::parse(text).map_err(|e| e.to_string())?.request;
    let globals = &state.globals;
    let taken = |name: &str| globals.iter().any(|v| v.name.as_str() == name);
    let mut imported = curl::import(text, taken).map_err(|e| e.to_string())?;
    let mut changes = Vec::new();
    let mut variables = Vec::new();

    // basic auth goes in a header, its encoded value kept as one secret
    let stored_auth = match (&literal.auth, &imported.request.auth) {
        (Auth::Basic { username, password }, Auth::Basic { .. }) => {
            imported
                .credentials
                .retain(|c| c.name.as_str() != "password");

            let name = curst_core::vars::VarName::from_hint("basic_auth");
            let value = curl::basic_value(username, password);
            let secret = value.strip_prefix("Basic ").unwrap_or(&value).to_owned();

            changes.push(SecretChange::Set(
                SecretKey::Global { name: name.clone() },
                curst_core::secrets::Secret::new(secret),
            ));
            variables.push(Variable::secret(name.clone()));

            Some(format!("Basic {{{{{name}}}}}"))
        }
        (_, Auth::Bearer { token }) => Some(format!("Bearer {token}")),
        _ => None,
    };
    let literal_auth = match &literal.auth {
        Auth::Basic { username, password } => Some(curl::basic_value(username, password)),
        Auth::Bearer { token } => Some(format!("Bearer {token}")),
        Auth::Inherit | Auth::None => None,
    };

    for credential in imported.credentials {
        variables.push(Variable::secret(credential.name.clone()));
        changes.push(SecretChange::Set(
            SecretKey::Global {
                name: credential.name,
            },
            credential.value,
        ));
    }

    let with_auth = |headers: &[KeyValue], auth: Option<String>| {
        let mut headers = headers.to_vec();

        if let Some(auth) = auth {
            headers.push(KeyValue::new("Authorization", auth));
        }

        headers
    };
    let stored = Source {
        location: curl::url_with_params(&imported.request.url, &imported.request.params),
        headers: with_auth(&imported.request.headers, stored_auth),
        picked: None,
    };
    let now = Source {
        location: curl::url_with_params(&literal.url, &literal.params),
        headers: with_auth(&literal.headers, literal_auth),
        picked: None,
    };

    Ok((stored, now, changes, variables))
}

/// keeps the secrets a curl command held as global ones, the variables
/// named in the globals, then runs `then`; `then` at once when it held none
pub(crate) fn keep_secrets(
    state: &mut State,
    changes: Vec<SecretChange>,
    variables: Vec<Variable>,
    then: Vec<Command>,
) -> Vec<Command> {
    if changes.is_empty() {
        return then;
    }

    let mut globals = state.globals.clone();

    for variable in variables {
        if !globals.iter().any(|v| v.name == variable.name) {
            globals.push(variable);
        }
    }

    let mut after = vec![Command::SaveGlobals(globals)];

    after.extend(then);

    super::send::change_secrets(state, changes, after)
}
