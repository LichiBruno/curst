//! curst: a terminal API client built on libcurl

#![cfg_attr(test, allow(clippy::panic))]

mod app;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use curst_core::http::{Cancel, Config, CurlExecutor, HttpExecutor};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};

use app::{Action, Command, Outcome, State};

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
    let mut terminal = ratatui::try_init().context("could not start the terminal UI")?;
    let result = Runtime::new(executor).run(&mut terminal);

    ratatui::try_restore().context("could not restore the terminal")?;

    result
}

struct Runtime {
    executor: Arc<dyn HttpExecutor>,
    results: Sender<Action>,
    inbox: Receiver<Action>,
    cancel: Option<Cancel>,
    workers: Vec<JoinHandle<()>>,
}

impl Runtime {
    fn new(executor: Arc<dyn HttpExecutor>) -> Self {
        let (results, inbox) = mpsc::channel();

        Self {
            executor,
            results,
            inbox,
            cancel: None,
            workers: Vec::new(),
        }
    }

    fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let mut state = State::default();
        let mut dirty = true;

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
        }

        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }

        // a cancelled transfer still owns its temporary body file until its
        // thread returns, so wait for every worker before leaving
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }

        Ok(())
    }

    fn execute(&mut self, command: Command, state: &State) {
        match command {
            Command::Send { id, request } => {
                let cancel = Cancel::new();
                let executor = Arc::clone(&self.executor);
                let results = self.results.clone();

                self.cancel = Some(cancel.clone());
                self.workers.retain(|worker| !worker.is_finished());
                self.workers.push(thread::spawn(move || {
                    let result = executor.execute(&request, &cancel);

                    let _ = results.send(Action::responded(id, result));
                }));
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
