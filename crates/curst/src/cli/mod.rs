//! the commands that run without the terminal UI, for scripts and CI:
//! sending a saved request, importing and exporting collections, and the
//! secrets curst keeps
//!
//! exit codes: 0 when it worked and a request was answered 1xx to 3xx, 1
//! when a request was answered 4xx or 5xx, 2 for a usage or setup problem,
//! 3 when a request could not be sent

mod run;
mod secrets;
mod transfer;

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Args, Subcommand};
use curst_core::http::{Config, CurlExecutor, HttpExecutor};
use curst_core::model::Variable;
use curst_core::secrets::{EncryptedFile, Keychain, Secret, SecretIndex, SecretStore, Secrets};
use curst_core::store::{Backend, Library, Paths, Settings};

use crate::app::{Action, Workspace};

/// the commands; their doc comments are the help text, so names are
/// written as people read them
#[allow(clippy::doc_markdown)]
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// send a saved request and print its answer: the body on stdout, the
    /// status on stderr
    Run(RunArgs),

    /// import an OpenAPI or Swagger spec (a file, a folder or a URL), or a
    /// bundle file, as a new collection
    Import(ImportArgs),

    /// write a collection as a bundle file, without its secrets' values
    Export(ExportArgs),

    /// the secrets curst keeps, by name; their values are never shown
    #[command(subcommand)]
    Secrets(SecretsCommand),
}

#[derive(Debug, Args)]
pub(crate) struct RunArgs {
    /// the request as `collection/folder/request`, by the names the
    /// sidebar shows (slugs work too)
    pub(crate) request: String,

    /// the environment to use, by name; the one chosen in curst otherwise
    #[arg(long, short, value_name = "NAME")]
    pub(crate) env: Option<String>,

    /// let the request's secrets go to this host, for this run only, though
    /// it was never approved for them
    #[arg(long, value_name = "HOST")]
    pub(crate) allow_host: Vec<String>,

    /// print status, headers, timings and body as one JSON object
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ImportArgs {
    /// a spec file, a folder holding specs, a URL (a spec or its docs
    /// page), or a `.curst.toml` bundle
    pub(crate) source: String,

    /// in a folder with several specs, the one to import, by its path in it
    #[arg(long, value_name = "PATH")]
    pub(crate) spec: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ExportArgs {
    /// the collection, by name
    pub(crate) collection: String,

    /// the bundle file to write, which must not exist yet
    pub(crate) file: PathBuf,

    /// write it even when it holds what looks like a credential in plain
    /// text
    #[arg(long)]
    pub(crate) force: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SecretsCommand {
    /// list every secret by name, with the hosts it may go to
    List,

    /// delete the secrets of collections this machine no longer knows
    Prune,
}

/// why a command stopped, and the exit code that says so
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Fail {
    pub(crate) code: u8,
    pub(crate) message: String,
}

impl Fail {
    /// a usage or setup problem
    pub(crate) fn setup(message: impl Into<String>) -> Self {
        Self {
            code: 2,
            message: message.into(),
        }
    }
}

/// runs `command`, printing why it failed on stderr
pub(crate) fn main(command: Command, config: Config) -> ExitCode {
    let library = Paths::from_env().map(Library::new);
    let result = library
        .map_err(|e| Fail::setup(format!("no data folder: {e}")))
        .and_then(|library| {
            let executor: Arc<dyn HttpExecutor> = Arc::new(CurlExecutor::new(config));

            dispatch(command, &library, executor.as_ref())
        });

    match result {
        Ok(code) => ExitCode::from(code),
        Err(fail) => {
            eprintln!("curst: {}", fail.message);

            ExitCode::from(fail.code)
        }
    }
}

fn dispatch(command: Command, library: &Library, executor: &dyn HttpExecutor) -> Result<u8, Fail> {
    let mut out = std::io::stdout().lock();
    let mut err = std::io::stderr().lock();
    let io = |e: std::io::Error| Fail::setup(format!("could not write the output: {e}"));

    match command {
        Command::Run(args) => {
            let answer = run::run(library, executor, &args, &|| passphrase(library.paths()))?;

            answer.print(&mut out, &mut err, args.json).map_err(io)?;

            Ok(answer.code())
        }
        Command::Import(args) => {
            let report = transfer::import(library, executor, &args)?;

            writeln!(err, "{report}").map_err(io)?;

            Ok(0)
        }
        Command::Export(args) => {
            let report = transfer::export(library, &args)?;

            writeln!(err, "{report}").map_err(io)?;

            Ok(0)
        }
        Command::Secrets(SecretsCommand::List) => {
            let list = secrets::list(library)?;

            out.write_all(list.as_bytes()).map_err(io)?;

            Ok(0)
        }
        Command::Secrets(SecretsCommand::Prune) => {
            let report = secrets::prune(library, &|| passphrase(library.paths()))?;

            writeln!(err, "{report}").map_err(io)?;

            Ok(0)
        }
    }
}

/// the collections and the global variables, as the UI loads them; the
/// `.curst/` folder in the current directory included
pub(crate) fn load(library: &Library) -> Result<(Vec<Workspace>, Vec<Variable>), Fail> {
    let cwd = std::env::current_dir().ok();

    match crate::storage::load_all(library, cwd.as_deref()) {
        Action::Loaded {
            collections,
            globals,
            ..
        } => Ok((collections, globals)),
        _ => Err(Fail::setup("the collections could not be read")),
    }
}

/// the collection called `name`, ignoring case
pub(crate) fn find<'a>(collections: &'a [Workspace], name: &str) -> Result<&'a Workspace, Fail> {
    collections
        .iter()
        .find(|w| w.collection.name.eq_ignore_ascii_case(name.trim()))
        .ok_or_else(|| {
            let names: Vec<&str> = collections
                .iter()
                .map(|w| w.collection.name.as_str())
                .collect();

            Fail::setup(format!(
                "no collection is called {name}; there are: {}",
                names.join(", ")
            ))
        })
}

/// the passphrase of the encrypted file: `CURST_PASSPHRASE`, or asked for
/// on the terminal without echo
pub(crate) type Ask<'a> = &'a dyn Fn() -> Result<Secret, Fail>;

fn passphrase(paths: &Paths) -> Result<Secret, Fail> {
    if let Ok(value) = std::env::var("CURST_PASSPHRASE") {
        return Ok(Secret::new(value));
    }

    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return Err(Fail::setup(format!(
            "the secrets are in the encrypted file {}, and there is no terminal to ask for its passphrase: set CURST_PASSPHRASE",
            paths.secrets_file().display()
        )));
    }

    read_hidden("passphrase: ").map(Secret::new)
}

/// a line typed on the terminal without echo
fn read_hidden(prompt: &str) -> Result<String, Fail> {
    use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use ratatui::crossterm::terminal;

    let fail = |e: std::io::Error| Fail::setup(format!("could not read the passphrase: {e}"));
    let mut err = std::io::stderr();

    write!(err, "{prompt}")
        .and_then(|()| err.flush())
        .map_err(fail)?;
    terminal::enable_raw_mode().map_err(fail)?;

    let mut text = String::new();
    let read = loop {
        match event::read() {
            Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Enter => break Ok(()),
                KeyCode::Esc => break Err(Fail::setup("no passphrase given")),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Err(Fail::setup("no passphrase given"));
                }
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Char(c) => text.push(c),
                _ => {}
            },
            Ok(_) => {}
            Err(e) => break Err(fail(e)),
        }
    };

    let _ = terminal::disable_raw_mode();
    let _ = writeln!(err);

    read.map(|()| text)
}

/// the secret store `config.toml` names, opened
pub(crate) fn open_store(paths: &Paths, ask: Ask<'_>) -> Result<Secrets, Fail> {
    let backend = Settings::load(&paths.config_file())
        .ok()
        .and_then(|(settings, _)| settings.secrets_backend);
    let store: Arc<dyn SecretStore> = match backend {
        None => {
            return Err(Fail::setup(
                "no secret store is chosen yet: open curst once to choose one",
            ));
        }
        Some(Backend::Keychain) => {
            let keychain = Keychain::open()
                .map_err(|e| Fail::setup(format!("the keychain could not be opened: {e}")))?;

            Arc::new(keychain)
        }
        Some(Backend::EncryptedFile) => Arc::new(EncryptedFile::new(paths.secrets_file(), ask()?)),
    };

    Ok(Secrets::new(
        SecretIndex::new(paths.secrets_index_file()),
        store,
    ))
}

#[cfg(test)]
mod tests;
