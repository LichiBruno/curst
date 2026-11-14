//! curst: a terminal API client built on libcurl

#![cfg_attr(test, allow(clippy::panic))]

mod app;
mod external;
mod session;
mod storage;
mod theme;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use curst_core::http::{Cancel, Config, CurlExecutor, HttpError, HttpExecutor, RedirectRule};
use curst_core::model::Request;
use curst_core::secrets::{SecretIndex, SecretKey};
use curst_core::store::{Library, Paths, Settings};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event,
};
use ratatui::crossterm::execute;

use app::{Action, Command, Outcome, SecretChange, State};
use session::{Job, Session};

/// how long a pause in typing is before edits are saved, in the
/// collections that save them without asking
const IDLE_PAUSE: Duration = Duration::from_millis(800);

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// seconds a request may take, 0 for no limit; overrides config.toml
    #[arg(long, value_name = "SECONDS")]
    timeout: Option<u64>,

    /// skip the TLS certificate and host name checks
    #[arg(short = 'k', long)]
    insecure: bool,

    /// trust the certificates in this PEM file instead of the system ones
    #[arg(long, value_name = "FILE")]
    cacert: Option<PathBuf>,

    /// use the standard keys, ignoring the [keys] section of config.toml
    #[arg(long)]
    default_keys: bool,
}

impl Cli {
    fn config(&self, settings: &Settings) -> Config {
        let defaults = Config::default();
        let timeout = match self.timeout.or(settings.http_timeout) {
            Some(0) => None,
            Some(seconds) => Some(Duration::from_secs(seconds)),
            None => defaults.timeout,
        };
        let body_cap = settings
            .http_max_body_mb
            .and_then(|mb| usize::try_from(mb).ok())
            .map_or(defaults.body_cap, |mb| mb.saturating_mul(1024 * 1024));

        Config {
            timeout,
            body_cap,
            insecure: self.insecure,
            ca_file: self.cacert.clone(),
            ..defaults
        }
    }
}

/// `config.toml` and what was wrong in it; a file that cannot be read at all
/// gives the defaults and one warning
fn load_settings(paths: Option<&Paths>) -> (Settings, Vec<String>) {
    let Some(paths) = paths else {
        return (Settings::default(), Vec::new());
    };

    Settings::load(&paths.config_file())
        .unwrap_or_else(|err| (Settings::default(), vec![err.to_string()]))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // without a data folder the app still opens, for one-off requests
    let library = Paths::from_env().map(Library::new);
    let paths = library.as_ref().ok().map(Library::paths);
    let (settings, warnings) = load_settings(paths);
    let executor: Arc<dyn HttpExecutor> = Arc::new(CurlExecutor::new(cli.config(&settings)));
    let startup = Startup::new(&cli, settings, warnings, paths);
    let mut terminal = ratatui::try_init().context("could not start the terminal UI")?;
    let modes = Modes::on(startup.settings.mouse != Some(false));

    modes.off_on_panic();

    let result = Runtime::new(executor, library, modes).run(&mut terminal, &startup);

    modes.off();
    ratatui::try_restore().context("could not restore the terminal")?;

    result
}

/// what `config.toml` set up before the terminal UI started
struct Startup {
    settings: Settings,
    warnings: Vec<String>,

    /// the preset and the user's bindings; with `--default-keys`, the
    /// standard preset and none
    keys_preset: String,
    key_bindings: Vec<(String, Vec<String>)>,
    theme: theme::Theme,

    /// the built-in themes and those in the themes folder
    themes: Vec<theme::Theme>,

    /// whether there is no `config.toml` yet, so the welcome is shown
    first_run: bool,
}

impl Startup {
    fn new(
        cli: &Cli,
        settings: Settings,
        mut warnings: Vec<String>,
        paths: Option<&Paths>,
    ) -> Self {
        let themes_dir = paths.map(Paths::themes_dir);
        let (theme, problems) =
            theme::Theme::load(settings.theme.as_deref(), themes_dir.as_deref());

        warnings.extend(problems);

        // the others are only reported when chosen
        let user = themes_dir.as_deref().map_or_else(Vec::new, |dir| {
            curst_core::store::theme_names(dir)
                .iter()
                .filter(|name| theme::Theme::built_in(name).is_none())
                .map(|name| theme::Theme::load(Some(name), Some(dir)).0)
                .collect()
        });
        let (keys_preset, key_bindings) = if cli.default_keys {
            ("standard".to_owned(), Vec::new())
        } else {
            (
                settings
                    .keys_preset
                    .clone()
                    .unwrap_or_else(|| "standard".to_owned()),
                settings.key_bindings.clone(),
            )
        };

        Self {
            first_run: paths.is_some_and(|p| !p.config_file().exists()),
            warnings,
            keys_preset,
            key_bindings,
            theme,
            themes: app::known_themes(user),
            settings,
        }
    }

    /// puts the preferences into `state`
    fn apply(&self, state: &mut State) -> Vec<String> {
        let preset = self.keys_preset.as_str();
        let (keymap, mut problems) = app::Keymap::from_settings(Some(preset), &self.key_bindings);
        let known = app::PRESETS.iter().any(|(name, _)| *name == preset);

        state.keymap = keymap;
        if known { preset } else { "standard" }.clone_into(&mut state.keys_preset);
        state.key_bindings.clone_from(&self.key_bindings);
        state.theme = self.theme.clone();
        state.sidebar_width = self.settings.sidebar_width;
        state.themes.clone_from(&self.themes);

        if !state.themes.iter().any(|t| t.name == self.theme.name) {
            state.themes.push(self.theme.clone());
        }

        let mut warnings = self.warnings.clone();

        warnings.append(&mut problems);

        warnings
    }
}

struct Runtime {
    executor: Arc<dyn HttpExecutor>,
    library: Result<Library, String>,
    results: Sender<Action>,
    inbox: Receiver<Action>,
    cancel: Option<Cancel>,

    /// the workflow step on its way, apart from the tab's request
    step_cancel: Option<Cancel>,
    workers: Vec<JoinHandle<()>>,

    /// the secrets thread's queue, and the thread
    secret_jobs: Option<Sender<Job>>,
    secrets_thread: Option<JoinHandle<()>>,

    /// a body to open in the user's editor once this round of commands is
    /// done, since that needs the terminal
    edit: Option<(String, &'static str)>,

    /// what curst switched on in the terminal
    modes: Modes,
}

impl Runtime {
    fn new(
        executor: Arc<dyn HttpExecutor>,
        library: Result<Library, curst_core::store::StoreError>,
        modes: Modes,
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
            step_cancel: None,
            workers: Vec::new(),
            secret_jobs,
            secrets_thread,
            edit: None,
            modes,
        }
    }

    fn run(mut self, terminal: &mut DefaultTerminal, setup: &Startup) -> Result<()> {
        let mut state = State::default();
        let warnings = setup.apply(&mut state);
        let mut dirty = true;
        let startup = match &self.library {
            Ok(library) => {
                let cwd = std::env::current_dir().ok();

                storage::load_all(library, cwd.as_deref())
            }
            Err(message) => Action::Notice(format!("collections are off: {message}")),
        };

        app::update(&mut state, startup);

        if self.library.is_ok() {
            app::update(
                &mut state,
                Action::BackendKnown(setup.settings.secrets_backend),
            );
        }

        if let Some(first) = warnings.first() {
            let more = warnings.len() - 1;
            let message = if more == 0 {
                format!("config.toml: {first}")
            } else {
                format!("config.toml: {first} (and {more} more)")
            };

            app::update(&mut state, Action::Notice(message));
        }

        if setup.first_run {
            app::update(&mut state, Action::Welcome);
        }

        // when the user last did something, and whether the pause after it
        // has yet to be reported
        let mut last_input = Instant::now();
        let mut idle_due = false;

        while !state.should_quit {
            if dirty {
                terminal.draw(|frame| ui::view(&state, frame))?;
                dirty = false;
            }

            let mut wait = Duration::from_millis(50);

            // every waiting event is taken at once, so mouse moves never
            // queue up ahead of keys; each one is read against the state the
            // one before it left
            while !state.should_quit && event::poll(wait)? {
                wait = Duration::ZERO;

                let actions = match event::read()? {
                    Event::Key(key) => app::action_for(&state, key),
                    Event::Paste(text) => vec![Action::PasteText(text)],
                    Event::Mouse(mouse) if self.modes.mouse => {
                        // a click lands on what is on the screen now
                        if dirty && app::mouse_acts(mouse) {
                            terminal.draw(|frame| ui::view(&state, frame))?;
                            dirty = false;
                        }

                        app::mouse_actions(&state, mouse)
                    }
                    Event::Resize(..) => {
                        dirty = true;

                        Vec::new()
                    }
                    _ => Vec::new(),
                };

                dirty |= !actions.is_empty();

                if !actions.is_empty() {
                    last_input = Instant::now();
                    idle_due = true;
                }

                for action in actions {
                    for command in app::update(&mut state, action) {
                        self.execute(command, &state);
                    }
                }
            }

            if idle_due && last_input.elapsed() >= IDLE_PAUSE {
                idle_due = false;

                for command in app::update(&mut state, Action::Idle) {
                    dirty = true;
                    self.execute(command, &state);
                }
            }

            let replies: Vec<Action> = self.inbox.try_iter().collect();

            dirty |= !replies.is_empty();

            for action in replies {
                for command in app::update(&mut state, action) {
                    self.execute(command, &state);
                }
            }

            if let Some((text, extension)) = self.edit.take() {
                let result = edit_in_terminal(terminal, &text, extension, self.modes)?;

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

        // a canceled transfer still owns its temporary body file until its
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

    /// puts `text` on the clipboard: through the terminal, and locally
    /// through the system too, since some terminals, such as Warp and
    /// Terminal.app, ignore OSC 52
    fn copy(&mut self, text: String) {
        copy_through_terminal(&text);

        if over_ssh(|key| std::env::var_os(key).is_some()) {
            return;
        }

        let results = self.results.clone();

        self.workers.push(thread::spawn(move || {
            if !copy_through_system(&text) {
                let _ = results.send(Action::Notice(
                    "the clipboard was not set: no clipboard tool worked, and this terminal may ignore OSC 52"
                        .into(),
                ));
            }
        }));
    }

    /// loads a collection's spec again from its source on a worker, for a
    /// refresh
    /// the check a spec fetch carrying `secrets` passes before it goes,
    /// as a send's does
    fn spec_fetch(
        &self,
        url: &str,
        headers: Vec<curst_core::model::KeyValue>,
        secrets: &[SecretKey],
        untrusted: bool,
    ) -> Result<SpecFetch, Stop> {
        let index = self
            .library
            .as_ref()
            .ok()
            .map(|l| SecretIndex::new(l.paths().secrets_index_file()));
        let request = Request {
            url: url.to_owned(),
            ..Request::default()
        };
        let rule = guard(index.as_ref(), secrets, &request, untrusted)?;

        Ok(SpecFetch {
            home: curst_core::url::origin(url),
            headers,
            rule,
        })
    }

    /// loads a collection's spec again from its source on a worker, for a
    /// refresh
    fn load_spec(
        &mut self,
        id: u64,
        collection: curst_core::model::LocalId,
        source: curst_core::model::Source,
        secrets: &[SecretKey],
        untrusted: bool,
    ) {
        use curst_core::openapi::{self, refresh::LoadError};

        let spec_fetch =
            match self.spec_fetch(&source.location, source.headers.clone(), secrets, untrusted) {
                Ok(spec_fetch) => spec_fetch,
                Err(stop) => {
                    let action = match stop {
                        Stop::Approve(approval) => Action::SpecNeedsApproval {
                            id,
                            approval: Box::new(approval),
                        },
                        Stop::Refused(message) => Action::SpecLoaded {
                            id,
                            collection,
                            result: Err(LoadError::Failed(message)),
                        },
                    };
                    let _ = self.results.send(action);

                    return;
                }
            };
        let executor = Arc::clone(&self.executor);
        let results = self.results.clone();

        self.workers.push(thread::spawn(move || {
            let cancel = Cancel::new();
            let fetch = |url: &str| spec_fetch.fetch(executor.as_ref(), url, &cancel);
            let result = openapi::refresh::load_source(&source, fetch).and_then(|(value, at)| {
                openapi::convert(&value, at.as_deref())
                    .map(Box::new)
                    .map_err(|e| LoadError::Failed(e.to_string()))
            });
            let _ = results.send(Action::SpecLoaded {
                id,
                collection,
                result,
            });
        }));
    }

    /// looks for the spec behind `url` on a worker, so the screen stays
    /// live while pages are fetched
    fn fetch_spec(
        &mut self,
        id: u64,
        url: String,
        headers: Vec<curst_core::model::KeyValue>,
        secrets: &[SecretKey],
        source: curst_core::model::Source,
    ) {
        let spec_fetch = match self.spec_fetch(&url, headers, secrets, false) {
            Ok(spec_fetch) => spec_fetch,
            Err(stop) => {
                let action = match stop {
                    Stop::Approve(approval) => Action::SpecNeedsApproval {
                        id,
                        approval: Box::new(approval),
                    },
                    Stop::Refused(message) => Action::SpecFetched {
                        id,
                        result: Err(message),
                    },
                };
                let _ = self.results.send(action);

                return;
            }
        };
        let executor = Arc::clone(&self.executor);
        let results = self.results.clone();

        self.workers.push(thread::spawn(move || {
            let result = fetch_spec(executor.as_ref(), &url, &spec_fetch, source);
            let _ = results.send(Action::SpecFetched { id, result });
        }));
    }

    /// sends a request on a worker, once its secrets pass the guard
    /// sends a workflow step's request through the same checks as any
    /// other, its answer going to the run, not a tab
    fn send_step(&mut self, id: u64, request: Request, secrets: Vec<SecretKey>, state: &State) {
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

        self.step_cancel = Some(cancel.clone());
        self.workers.retain(|worker| !worker.is_finished());
        self.workers.push(thread::spawn(move || {
            let action = match guard(index.as_ref(), &secrets, &request, untrusted) {
                Ok(rule) => Action::StepAnswered {
                    id,
                    result: Box::new(executor.execute(&request, &cancel, &rule)),
                },
                Err(Stop::Refused(message)) => Action::StepAnswered {
                    id,
                    result: Box::new(Err(HttpError::Transfer(message))),
                },
                Err(Stop::Approve(approval)) => Action::StepNeedsApproval {
                    id,
                    approval: Box::new(approval),
                },
            };

            let _ = results.send(action);
        }));
    }

    fn send(&mut self, id: u64, request: Request, secrets: Vec<SecretKey>, state: &State) {
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
                Ok(rule) => Action::responded(id, executor.execute(&request, &cancel, &rule)),
                Err(Stop::Refused(message)) => {
                    Action::responded(id, Err(HttpError::Transfer(message)))
                }
                Err(Stop::Approve(approval)) => Action::NeedsApproval {
                    id,
                    approval: Box::new(approval),
                },
            };

            let _ = results.send(action);
        }));
    }

    fn execute(&mut self, command: Command, state: &State) {
        match command {
            Command::Send {
                id,
                request,
                secrets,
            } => self.send(id, request, secrets, state),
            Command::SendStep {
                id,
                request,
                secrets,
            } => self.send_step(id, request, secrets, state),
            Command::CancelStep => {
                if let Some(cancel) = self.step_cancel.take() {
                    cancel.cancel();
                }
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
            Command::FetchSpec {
                id,
                url,
                headers,
                secrets,
                source,
            } => self.fetch_spec(id, url, headers, &secrets, source),
            Command::LoadSpec {
                id,
                collection,
                source,
                secrets,
            } => {
                let untrusted = state.workspace(&collection).is_none_or(|w| !w.trusted);

                self.load_spec(id, collection, source, &secrets, untrusted);
            }
            Command::Copy(text) => self.copy(text),
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
/// the keyboard protocol that tells keys like Ctrl+1 apart from plain 1,
/// switched on only where the terminal supports it
/// the terminal modes curst switches on: the keyboard protocol, the mouse
/// and bracketed paste. they are switched off whenever the terminal is handed back, at
/// exit, around the editor and on a crash, or the shell would be left
/// printing escape codes for every key and mouse move
#[derive(Debug, Clone, Copy)]
struct Modes {
    enhanced: bool,
    mouse: bool,

    /// pasted text arrives as one event; without it, a pasted newline
    /// would press Enter
    paste: bool,
}

impl Modes {
    fn on(mouse: bool) -> Self {
        let enhanced = keyboard::enable();
        let mouse = mouse && execute!(std::io::stdout(), EnableMouseCapture).is_ok();
        let paste = execute!(std::io::stdout(), EnableBracketedPaste).is_ok();

        Self {
            enhanced,
            mouse,
            paste,
        }
    }

    fn off(self) {
        if self.mouse {
            let _ = execute!(std::io::stdout(), DisableMouseCapture);
        }

        if self.paste {
            let _ = execute!(std::io::stdout(), DisableBracketedPaste);
        }

        keyboard::disable(self.enhanced);
    }

    fn resume(self) {
        if self.enhanced {
            keyboard::enable();
        }

        if self.mouse {
            let _ = execute!(std::io::stdout(), EnableMouseCapture);
        }

        if self.paste {
            let _ = execute!(std::io::stdout(), EnableBracketedPaste);
        }
    }

    /// switches the modes off before a crash is reported; ratatui's own
    /// hook, which runs next, restores the screen
    fn off_on_panic(self) {
        let previous = std::panic::take_hook();

        std::panic::set_hook(Box::new(move |info| {
            self.off();
            previous(info);
        }));
    }
}

/// the spec behind a URL, found and converted for a preview; the source
/// kept is the URL as given, so a page that moves its spec still works
fn fetch_spec(
    executor: &dyn HttpExecutor,
    url: &str,
    spec_fetch: &SpecFetch,
    source: curst_core::model::Source,
) -> Result<Box<app::SpecPreview>, String> {
    use curst_core::openapi;

    let cancel = Cancel::new();
    let (value, found_at) = openapi::discover(url, |at| spec_fetch.fetch(executor, at, &cancel))?;
    let converted = openapi::convert(&value, Some(&found_at)).map_err(|e| e.to_string())?;

    Ok(Box::new(app::SpecPreview::new(converted, source)))
}

/// `bytes` in base64, as OSC 52 carries them
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);

        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }

    out
}

/// asks the terminal to put `text` on the clipboard, with OSC 52: it
/// works over SSH too, but a terminal that does not allow it says nothing
fn copy_through_terminal(text: &str) {
    use std::io::Write;

    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()));
    let _ = out.flush();
}

/// whether curst runs on another machine than the one the user sits at,
/// where its system clipboard would be the wrong one
fn over_ssh(set: impl Fn(&str) -> bool) -> bool {
    set("SSH_CONNECTION") || set("SSH_TTY")
}

/// the programs that set the system clipboard from their input, to try in
/// order
fn clipboard_tools(wayland: bool) -> Vec<(&'static str, &'static [&'static str])> {
    if cfg!(target_os = "macos") {
        return vec![("pbcopy", &[])];
    }

    if cfg!(windows) {
        return vec![("clip", &[])];
    }

    let mut tools: Vec<(&str, &[&str])> = Vec::new();

    if wayland {
        tools.push(("wl-copy", &[]));
    }

    tools.push(("xclip", &["-selection", "clipboard"]));
    tools.push(("xsel", &["--clipboard", "--input"]));

    tools
}

/// puts `text` on the system clipboard through its input, never its
/// arguments, with the first tool that works; false when none did
fn copy_through_system(text: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();

    for (program, args) in clipboard_tools(wayland) {
        let child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            continue;
        };

        let wrote = child
            .stdin
            .take()
            .is_some_and(|mut input| input.write_all(text.as_bytes()).is_ok());

        if child.wait().is_ok_and(|status| status.success()) && wrote {
            return true;
        }
    }

    false
}

mod keyboard {
    use ratatui::crossterm::event::{
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    };
    use ratatui::crossterm::execute;
    use ratatui::crossterm::terminal::supports_keyboard_enhancement;

    /// switches the protocol on; returns whether it is on
    pub(super) fn enable() -> bool {
        matches!(supports_keyboard_enhancement(), Ok(true))
            && execute!(
                std::io::stdout(),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )
            .is_ok()
    }

    pub(super) fn disable(enabled: bool) {
        if enabled {
            let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
        }
    }
}

fn edit_in_terminal(
    terminal: &mut DefaultTerminal,
    text: &str,
    extension: &str,
    modes: Modes,
) -> Result<Result<String, String>> {
    use ratatui::crossterm::execute;
    use ratatui::crossterm::terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    };

    // the same terminal is kept, only handed over for a while
    modes.off();
    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    let command = external::editor_command(|key| std::env::var(key).ok());
    let result = external::edit(text, extension, &command);

    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;

    modes.resume();
    terminal.clear()?;

    Ok(result)
}

/// why a request carrying secrets must not go yet
enum Stop {
    /// never, and why
    Refused(String),

    /// not before the user approves the host
    Approve(curst_core::secrets::Approval),
}

/// the redirect rule for a request carrying `secrets`, checked just
/// before it goes: or, when it must not go, why
fn guard(
    index: Option<&SecretIndex>,
    secrets: &[SecretKey],
    request: &Request,
    untrusted: bool,
) -> Result<RedirectRule, Stop> {
    if secrets.is_empty() {
        return Ok(RedirectRule::Any);
    }

    if untrusted {
        return Err(Stop::Refused(
            "a secret of a collection you have not trusted is never sent".into(),
        ));
    }

    let Some(index) = index else {
        return Err(Stop::Refused("secrets need a data folder".into()));
    };

    match index.unapproved(secrets, &request.full_url()) {
        Ok(None) => index
            .redirect_rule(secrets)
            .map_err(|e| Stop::Refused(e.to_string())),
        Ok(Some(approval)) => Err(Stop::Approve(approval)),
        Err(err) => Err(Stop::Refused(err.to_string())),
    }
}

/// how a spec is fetched: its headers go only to the origin of the URL it
/// was given, never to the other pages discovery looks at, and follow only
/// the redirects its secrets allow
struct SpecFetch {
    home: Option<curst_core::url::Origin>,
    headers: Vec<curst_core::model::KeyValue>,
    rule: RedirectRule,
}

impl SpecFetch {
    fn fetch(
        &self,
        executor: &dyn HttpExecutor,
        url: &str,
        cancel: &Cancel,
    ) -> Result<curst_core::openapi::Fetched, String> {
        let home = self.home.is_some() && curst_core::url::origin(url) == self.home;

        if home {
            curst_core::openapi::fetch_with(executor, url, &self.headers, cancel, &self.rule)
        } else {
            curst_core::openapi::fetch_with(executor, url, &[], cancel, &RedirectRule::Any)
        }
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

        fn outcome(result: Result<RedirectRule, Stop>) -> String {
            match result {
                Ok(rule) => format!("{rule:?}"),
                Err(Stop::Approve(approval)) => format!("ask {}", approval.origin),
                Err(Stop::Refused(message)) => format!("refused: {message}"),
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
    fn startup_reads_the_preferences_and_user_themes() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let settings = Settings {
            keys_preset: Some("vim".into()),
            theme: Some("night".into()),
            key_bindings: vec![("global.send".into(), vec!["f5".into()])],
            ..Settings::default()
        };

        std::fs::create_dir_all(paths.themes_dir()).unwrap();
        std::fs::write(paths.themes_dir().join("night.toml"), "accent = \"red\"\n").unwrap();
        std::fs::write(paths.themes_dir().join("day.toml"), "base = \"light\"\n").unwrap();

        let cli = Cli::try_parse_from(["curst"]).unwrap();
        let startup = Startup::new(&cli, settings.clone(), Vec::new(), Some(&paths));
        let mut state = State::default();
        let warnings = startup.apply(&mut state);
        let names: Vec<&str> = state.themes.iter().map(|t| t.name.as_str()).collect();

        assert!(startup.first_run);
        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(names, ["dark", "light", "mono", "day", "night"]);
        assert_eq!(state.theme.name, "night");
        assert_eq!(state.keys_preset, "vim");
        assert_eq!(state.key(app::Layer::Global, "send"), "F5");

        // --default-keys is the way back from keys that broke everything
        let cli = Cli::try_parse_from(["curst", "--default-keys"]).unwrap();
        let mut state = State::default();

        std::fs::write(paths.config_file(), "").unwrap();

        let startup = Startup::new(&cli, settings, Vec::new(), Some(&paths));

        startup.apply(&mut state);

        assert!(!startup.first_run);
        assert_eq!(state.keys_preset, "standard");
        assert_eq!(state.key(app::Layer::Global, "send"), "Ctrl+R");
    }

    #[test]
    fn the_system_clipboard_is_for_local_sessions_only() {
        assert!(over_ssh(|key| key == "SSH_CONNECTION"));
        assert!(over_ssh(|key| key == "SSH_TTY"));
        assert!(!over_ssh(|_| false));

        let tools: Vec<&str> = clipboard_tools(true).iter().map(|(p, _)| *p).collect();

        if cfg!(target_os = "macos") {
            assert_eq!(tools, ["pbcopy"]);
        } else if cfg!(windows) {
            assert_eq!(tools, ["clip"]);
        } else {
            assert_eq!(tools, ["wl-copy", "xclip", "xsel"]);
        }
    }

    /// a web of fixed pages; an address it does not have answers 404, and
    /// `/old` redirects to the page
    struct Pages {
        pages: Vec<(&'static str, &'static str)>,

        /// every address asked for, and whether an Authorization header
        /// went with it
        asked: std::sync::Mutex<Vec<(String, bool)>>,
    }

    fn web(pages: Vec<(&'static str, &'static str)>) -> Pages {
        Pages {
            pages,
            asked: std::sync::Mutex::new(Vec::new()),
        }
    }

    impl HttpExecutor for Pages {
        fn execute(
            &self,
            request: &Request,
            _: &Cancel,
            _: &RedirectRule,
        ) -> Result<curst_core::http::Response, HttpError> {
            let url = request.url.replace("/old", "/docs/");
            let authorized = request
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("authorization"));
            let mut response = app::tests::response("");

            if let Ok(mut asked) = self.asked.lock() {
                asked.push((url.clone(), authorized));
            }

            if let Some((_, body)) = self.pages.iter().find(|(at, _)| *at == url) {
                response = app::tests::response(body);
            } else {
                response.status = 404;
                response.status_line = "HTTP/1.1 404 Not Found".into();
            }

            response.effective_url = url;

            Ok(response)
        }
    }

    fn plain(url: &str) -> (SpecFetch, curst_core::model::Source) {
        (
            SpecFetch {
                home: curst_core::url::origin(url),
                headers: Vec::new(),
                rule: RedirectRule::Any,
            },
            curst_core::model::Source {
                location: url.to_owned(),
                ..curst_core::model::Source::default()
            },
        )
    }

    #[test]
    fn a_spec_is_found_behind_its_page_and_keeps_the_url_given() {
        let pages = web(vec![
            (
                "https://h/docs/",
                include_str!("../../curst-core/src/openapi/fixtures/swagger-ui.html"),
            ),
            (
                "https://h/docs/swagger-initializer.js",
                include_str!("../../curst-core/src/openapi/fixtures/swagger-initializer.js"),
            ),
            (
                "https://h/v1/openapi.yaml",
                include_str!("../../curst-core/src/openapi/fixtures/notes-3.1.yaml"),
            ),
        ]);
        let (fetch, source) = plain("https://h/old");
        let preview = fetch_spec(&pages, "https://h/old", &fetch, source).unwrap();
        let base = preview.converted.bundle.environments[0]
            .1
            .iter()
            .find(|v| v.name.as_str() == "base_url")
            .unwrap();

        // the URL as given is kept, so a page that moves its spec still works
        assert_eq!(preview.source.location, "https://h/old");
        assert_eq!(base.value, "https://h/api");

        let (fetch, source) = plain("https://h/none");

        assert!(fetch_spec(&web(vec![]), "https://h/none", &fetch, source).is_err());
    }

    #[test]
    fn spec_headers_go_only_to_the_origin_they_were_given_for() {
        let pages = web(vec![
            (
                "https://h/docs/",
                "<script>SwaggerUIBundle({ url: \"https://cdn.example/spec.yaml\" })</script>",
            ),
            (
                "https://cdn.example/spec.yaml",
                include_str!("../../curst-core/src/openapi/fixtures/notes-3.1.yaml"),
            ),
        ]);
        let fetch = SpecFetch {
            home: curst_core::url::origin("https://h/docs/"),
            headers: vec![curst_core::model::KeyValue::new(
                "Authorization",
                "Bearer t0k",
            )],
            rule: RedirectRule::Any,
        };
        let (_, source) = plain("https://h/docs/");

        fetch_spec(&pages, "https://h/docs/", &fetch, source).unwrap();

        assert_eq!(
            *pages.asked.lock().unwrap(),
            [
                ("https://h/docs/".to_owned(), true),
                ("https://cdn.example/spec.yaml".to_owned(), false),
            ]
        );
    }

    #[test]
    fn a_workflow_step_asks_before_its_secrets_go_and_answers_apart() {
        use curst_core::secrets::{MemoryStore, Secret, Secrets};

        let dir = tempfile::tempdir().unwrap();
        let library = Library::new(Paths::under(dir.path()));
        let index = SecretIndex::new(library.paths().secrets_index_file());
        let token = SecretKey::Global {
            name: curst_core::vars::VarName::new("token").unwrap(),
        };

        Secrets::new(index.clone(), Arc::new(MemoryStore::new()))
            .set(&token, &Secret::new("t"))
            .unwrap();

        let mut runtime = Runtime::new(
            Arc::new(web(vec![("https://api.example.com/login", "{}")])),
            Ok(library),
            Modes {
                enhanced: false,
                mouse: false,
                paste: false,
            },
        );
        let state = State::default();
        let step = || Command::SendStep {
            id: 7,
            request: Request {
                url: "https://api.example.com/login".into(),
                ..Request::default()
            },
            secrets: vec![token.clone()],
        };
        let reply = |runtime: &Runtime| runtime.inbox.recv_timeout(Duration::from_secs(5)).unwrap();

        // an origin not approved for the secret: the run is asked, nothing goes
        runtime.execute(step(), &state);

        assert!(matches!(
            reply(&runtime),
            Action::StepNeedsApproval { id: 7, .. }
        ));

        // once approved, the answer comes back to the run, not to a tab
        index
            .approve(std::slice::from_ref(&token), "https://api.example.com")
            .unwrap();
        runtime.execute(step(), &state);

        assert!(matches!(
            reply(&runtime),
            Action::StepAnswered { id: 7, result } if result.as_ref().as_ref().is_ok_and(|r| r.status == 200)
        ));
    }

    #[test]
    fn base64_is_what_osc_52_expects() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64("curl é".as_bytes()), "Y3VybCDDqQ==");
    }

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn defaults_keep_the_core_settings() {
        let config = Cli::try_parse_from(["curst"])
            .unwrap()
            .config(&Settings::default());

        assert_eq!(config, Config::default());
    }

    #[test]
    fn config_toml_fills_the_config_and_flags_win() {
        let settings = Settings {
            http_timeout: Some(0),
            http_max_body_mb: Some(2),
            ..Settings::default()
        };
        let config = Cli::try_parse_from(["curst"]).unwrap().config(&settings);

        assert_eq!(config.timeout, None);
        assert_eq!(config.body_cap, 2 * 1024 * 1024);

        let config = Cli::try_parse_from(["curst", "--timeout", "7"])
            .unwrap()
            .config(&settings);

        assert_eq!(config.timeout, Some(Duration::from_secs(7)));
    }

    #[test]
    fn flags_fill_the_config() {
        let config = Cli::try_parse_from(["curst", "-k", "--timeout", "0", "--cacert", "ca.pem"])
            .unwrap()
            .config(&Settings::default());

        assert_eq!(config.timeout, None);
        assert!(config.insecure);
        assert_eq!(config.ca_file, Some(PathBuf::from("ca.pem")));

        let config = Cli::try_parse_from(["curst", "--timeout", "5"])
            .unwrap()
            .config(&Settings::default());

        assert_eq!(config.timeout, Some(Duration::from_secs(5)));
    }
}
