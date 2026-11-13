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
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use curst_core::http::{Cancel, Config, CurlExecutor, HttpError, HttpExecutor, RedirectRule};
use curst_core::model::Request;
use curst_core::secrets::{SecretIndex, SecretKey};
use curst_core::store::{Library, Paths, Settings};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use ratatui::crossterm::execute;

use app::{Action, Command, Outcome, SecretChange, State};
use session::{Job, Session};

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

                for action in actions {
                    for command in app::update(&mut state, action) {
                        self.execute(command, &state);
                    }
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
/// the keyboard protocol that tells keys like Ctrl+1 apart from plain 1,
/// switched on only where the terminal supports it
/// the terminal modes curst switches on: the keyboard protocol and the
/// mouse. they are switched off whenever the terminal is handed back, at
/// exit, around the editor and on a crash, or the shell would be left
/// printing escape codes for every key and mouse move
#[derive(Debug, Clone, Copy)]
struct Modes {
    enhanced: bool,
    mouse: bool,
}

impl Modes {
    fn on(mouse: bool) -> Self {
        let enhanced = keyboard::enable();
        let mouse = mouse && execute!(std::io::stdout(), EnableMouseCapture).is_ok();

        Self { enhanced, mouse }
    }

    fn off(self) {
        if self.mouse {
            let _ = execute!(std::io::stdout(), DisableMouseCapture);
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
