//! curst: a terminal API client built on libcurl

#![cfg_attr(test, allow(clippy::panic))]

mod app;
mod external;
mod session;
mod storage;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use curst_core::http::{Cancel, Config, CurlExecutor, HttpError, HttpExecutor, RedirectRule};
use curst_core::model::Request;
use curst_core::secrets::{SecretIndex, SecretKey};
use curst_core::store::{Library, Paths};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};

use app::{Action, Command, Outcome, SecretChange, State};
use session::{Job, Session};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// seconds a request may take
    #[arg(long, value_name = "SECONDS", default_value_t = 30)]
    timeout: u64,

    /// skip the TLS certificate and host name checks
    #[arg(short = 'k', long)]
    insecure: bool,

    /// trust the certificates in this PEM file instead of the system ones
    #[arg(long, value_name = "FILE")]
    cacert: Option<PathBuf>,
}

impl Cli {
    fn config(self) -> Config {
        Config {
            timeout: (self.timeout > 0).then(|| Duration::from_secs(self.timeout)),
            insecure: self.insecure,
            ca_file: self.cacert,
            ..Config::default()
        }
    }
}

fn main() -> Result<()> {
    let config = Cli::parse().config();
    let executor: Arc<dyn HttpExecutor> = Arc::new(CurlExecutor::new(config));
    // without a data folder the app still opens, for one-off requests
    let library = Paths::from_env().map(Library::new);
    let mut terminal = ratatui::try_init().context("could not start the terminal UI")?;
    let result = Runtime::new(executor, library).run(&mut terminal);

    ratatui::try_restore().context("could not restore the terminal")?;

    result
}

struct Runtime {
    executor: Arc<dyn HttpExecutor>,
    library: Result<Library, String>,
    results: Sender<Action>,
    inbox: Receiver<Action>,
    cancel: Option<Cancel>,
    workers: Vec<JoinHandle<()>>,

    /// the secrets thread's queue, and the thread
    secret_jobs: Option<Sender<Job>>,
    secrets_thread: Option<JoinHandle<()>>,

    /// a body to open in the user's editor once this round of commands is
    /// done, since that needs the terminal
    edit: Option<(String, &'static str)>,
}

impl Runtime {
    fn new(
        executor: Arc<dyn HttpExecutor>,
        library: Result<Library, curst_core::store::StoreError>,
    ) -> Self {
        let (results, inbox) = mpsc::channel();
        let mut secret_jobs = None;
        let mut secrets_thread = None;

        if let Ok(library) = &library {
            let mut session = Session::new(library.paths());
            let (jobs, queue) = mpsc::channel::<Job>();
            let replies = results.clone();

            secret_jobs = Some(jobs);
            secrets_thread = Some(thread::spawn(move || {
                for job in queue {
                    let _ = replies.send(session.run(job));
                }
            }));
        }

        Self {
            executor,
            library: library.map_err(|e| e.to_string()),
            results,
            inbox,
            cancel: None,
            workers: Vec::new(),
            secret_jobs,
            secrets_thread,
            edit: None,
        }
    }

    fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let mut state = State::default();
        let mut dirty = true;
        let startup = match &self.library {
            Ok(library) => {
                let cwd = std::env::current_dir().ok();

                storage::load_all(library, cwd.as_deref())
            }
            Err(message) => Action::Notice(format!("collections are off: {message}")),
        };

        app::update(&mut state, startup);

        if let Ok(library) = &self.library {
            let backend = curst_core::store::Settings::load(&library.paths().config_file())
                .ok()
                .and_then(|s| s.secrets_backend);

            app::update(&mut state, Action::BackendKnown(backend));
        }

        while !state.should_quit {
            if dirty {
                terminal.draw(|frame| ui::view(&state, frame))?;
                dirty = false;
            }

            let mut actions = Vec::new();

            if event::poll(Duration::from_millis(50))? {
                dirty = true;

                if let Event::Key(key) = event::read()?
                    && let Some(action) = app::action_for(&state, key)
                {
                    actions.push(action);
                }
            }

            actions.extend(self.inbox.try_iter());
            dirty |= !actions.is_empty();

            for action in actions {
                for command in app::update(&mut state, action) {
                    self.execute(command, &state);
                }
            }

            if let Some((text, extension)) = self.edit.take() {
                let result = edit_in_terminal(terminal, &text, extension)?;

                app::update(&mut state, Action::BodyEdited(result));
                dirty = true;
            }
        }

        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }

        // closing the queue ends the secrets thread once its job is done
        self.secret_jobs = None;

        if let Some(thread) = self.secrets_thread.take() {
            let _ = thread.join();
        }

        // a cancelled transfer still owns its temporary body file until its
        // thread returns, so wait for every worker before leaving
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }

        Ok(())
    }

    fn secret_job(&self, job: Job) {
        let sent = self
            .secret_jobs
            .as_ref()
            .is_some_and(|jobs| jobs.send(job).is_ok());

        if !sent {
            let _ = self
                .results
                .send(Action::Notice("secrets need a data folder".into()));
        }
    }

    fn execute(&mut self, command: Command, state: &State) {
        match command {
            Command::Send {
                id,
                request,
                secrets,
            } => {
                let cancel = Cancel::new();
                let executor = Arc::clone(&self.executor);
                let results = self.results.clone();
                let index = self
                    .library
                    .as_ref()
                    .ok()
                    .map(|l| SecretIndex::new(l.paths().secrets_index_file()));
                let untrusted = secrets.iter().any(|key| {
                    key.collection()
                        .is_some_and(|c| state.workspace(c).is_none_or(|w| !w.trusted))
                });

                self.cancel = Some(cancel.clone());
                self.workers.retain(|worker| !worker.is_finished());
                self.workers.push(thread::spawn(move || {
                    let action = match guard(index.as_ref(), &secrets, &request, untrusted) {
                        Ok(rule) => {
                            Action::responded(id, executor.execute(&request, &cancel, &rule))
                        }
                        Err(action) => action(id),
                    };

                    let _ = results.send(action);
                }));
            }
            Command::LoadSecrets { id, keys } => self.secret_job(Job::Load { id, keys }),
            Command::Unlock(passphrase) => self.secret_job(Job::Unlock(passphrase)),
            Command::ChooseBackend(backend) => self.secret_job(Job::Choose(backend)),
            Command::Approve { id, keys, origin } => {
                self.secret_job(Job::Approve { id, keys, origin });
            }
            Command::Migrate { id, to, passphrase } => {
                self.secret_job(Job::Migrate { id, to, passphrase });
            }
            Command::EditBody { text, extension } => self.edit = Some((text, extension)),
            Command::ChangeSecrets { id, changes } => {
                // a secret of an untrusted collection is never stored
                let untrusted = changes.iter().any(|change| {
                    let key = match change {
                        SecretChange::Set(key, _)
                        | SecretChange::Rename(key, _)
                        | SecretChange::Delete(key) => key.collection(),
                        SecretChange::DeleteScope(_) => None,
                    };

                    key.is_some_and(|c| state.workspace(c).is_some_and(|w| !w.trusted))
                });

                if untrusted {
                    let _ = self.results.send(Action::SecretsChanged {
                        id,
                        result: Err("trust this collection before keeping secrets for it".into()),
                    });
                } else {
                    self.secret_job(Job::Change { id, changes });
                }
            }
            Command::Cancel => {
                if let Some(cancel) = self.cancel.take() {
                    cancel.cancel();
                }
            }
            Command::SaveBody(path) => {
                let result = match &state.outcome {
                    Some(Outcome::Response(view)) => view
                        .response
                        .body
                        .save_to(&path)
                        .map(|()| path.clone())
                        .map_err(|err| save_error(&path, &err)),
                    _ => Err("nothing to save".into()),
                };

                let _ = self.results.send(Action::Saved(result));
            }
            command => {
                let action = match &self.library {
                    Ok(library) => storage::run(library, &state.collections, command),
                    Err(message) => Some(Action::Notice(format!("collections are off: {message}"))),
                };

                if let Some(action) = action {
                    let _ = self.results.send(action);
                }
            }
        }
    }
}

/// hands the terminal to the user's editor and takes it back after
fn edit_in_terminal(
    terminal: &mut DefaultTerminal,
    text: &str,
    extension: &str,
) -> Result<Result<String, String>> {
    use ratatui::crossterm::execute;
    use ratatui::crossterm::terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    };

    // the same terminal is kept, only handed over for a while
    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    let command = external::editor_command(|key| std::env::var(key).ok());
    let result = external::edit(text, extension, &command);

    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    terminal.clear()?;

    Ok(result)
}

/// the redirect rule for a request carrying `secrets`, checked just
/// before it goes: or, when it must not go, the action that says why
fn guard(
    index: Option<&SecretIndex>,
    secrets: &[SecretKey],
    request: &Request,
    untrusted: bool,
) -> Result<RedirectRule, Box<dyn FnOnce(u64) -> Action + Send>> {
    let refuse = |message: String| -> Box<dyn FnOnce(u64) -> Action + Send> {
        Box::new(move |id| Action::responded(id, Err(HttpError::Transfer(message))))
    };

    if secrets.is_empty() {
        return Ok(RedirectRule::Any);
    }

    if untrusted {
        return Err(refuse(
            "a secret of a collection you have not trusted is never sent".into(),
        ));
    }

    let Some(index) = index else {
        return Err(refuse("secrets need a data folder".into()));
    };

    match index.unapproved(secrets, &request.full_url()) {
        Ok(None) => index
            .redirect_rule(secrets)
            .map_err(|e| refuse(e.to_string())),
        Ok(Some(approval)) => Err(Box::new(move |id| Action::NeedsApproval {
            id,
            approval: Box::new(approval),
        })),
        Err(err) => Err(refuse(err.to_string())),
    }
}

fn save_error(path: &std::path::Path, err: &std::io::Error) -> String {
    if err.kind() == std::io::ErrorKind::AlreadyExists {
        format!("{} already exists, choose another name", path.display())
    } else {
        format!("could not save {}: {err}", path.display())
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    mod guard {
        use std::sync::Arc;

        use curst_core::model::LocalId;
        use curst_core::secrets::{MemoryStore, Secret, Secrets};
        use curst_core::vars::VarName;

        use super::super::*;

        fn request(url: &str) -> Request {
            Request {
                url: url.into(),
                ..Request::default()
            }
        }

        fn outcome(result: Result<RedirectRule, Box<dyn FnOnce(u64) -> Action + Send>>) -> String {
            match result {
                Ok(rule) => format!("{rule:?}"),
                Err(action) => match action(1) {
                    Action::NeedsApproval { approval, .. } => format!("ask {}", approval.origin),
                    Action::Responded { result, .. } => match *result {
                        Err(err) => format!("refused: {err}"),
                        Ok(_) => "sent".into(),
                    },
                    other => format!("{other:?}"),
                },
            }
        }

        #[test]
        fn secrets_go_only_where_approved() {
            let dir = tempfile::tempdir().unwrap();
            let index = SecretIndex::new(dir.path().join("secrets-index.toml"));
            let secrets = Secrets::new(index.clone(), Arc::new(MemoryStore::new()));
            let global = SecretKey::Global {
                name: VarName::new("token").unwrap(),
            };
            let keys = std::slice::from_ref(&global);

            secrets.set(&global, &Secret::new("t")).unwrap();

            // a global secret, used from a trusted collection: the first send
            // asks
            assert_eq!(
                outcome(guard(
                    Some(&index),
                    keys,
                    &request("https://api.example.com/x"),
                    false
                )),
                "ask https://api.example.com:443"
            );

            index.approve(keys, "https://api.example.com").unwrap();

            assert!(
                outcome(guard(
                    Some(&index),
                    keys,
                    &request("https://api.example.com/y"),
                    false
                ))
                .starts_with("Within")
            );

            // a new origin asks again, plain http included
            assert_eq!(
                outcome(guard(
                    Some(&index),
                    keys,
                    &request("http://api.example.com/y"),
                    false
                )),
                "ask http://api.example.com:80"
            );

            // no secrets, no question
            assert_eq!(outcome(guard(None, &[], &request("x"), true)), "Any");

            // an untrusted collection's secrets never go, approved or not
            let theirs = SecretKey::Collection {
                collection: LocalId::new("c2").unwrap(),
                name: VarName::new("k").unwrap(),
            };

            assert!(
                outcome(guard(
                    Some(&index),
                    &[theirs],
                    &request("https://api.example.com/"),
                    true
                ))
                .contains("never sent")
            );
            assert!(
                outcome(guard(None, keys, &request("https://h/"), false)).contains("data folder")
            );
            assert!(
                outcome(guard(Some(&index), keys, &request("h"), false)).starts_with("refused")
            );
        }
    }

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn defaults_keep_the_core_settings() {
        let config = Cli::try_parse_from(["curst"]).unwrap().config();

        assert_eq!(config, Config::default());
    }

    #[test]
    fn flags_fill_the_config() {
        let config = Cli::try_parse_from(["curst", "-k", "--timeout", "0", "--cacert", "ca.pem"])
            .unwrap()
            .config();

        assert_eq!(config.timeout, None);
        assert!(config.insecure);
        assert_eq!(config.ca_file, Some(PathBuf::from("ca.pem")));

        let config = Cli::try_parse_from(["curst", "--timeout", "5"])
            .unwrap()
            .config();

        assert_eq!(config.timeout, Some(Duration::from_secs(5)));
    }
}
