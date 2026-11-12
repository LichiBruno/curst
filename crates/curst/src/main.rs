//! curst: a terminal API client built on libcurl

#![cfg_attr(test, allow(clippy::panic))]

mod app;
mod ui;

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use curst_core::http::{Cancel, CurlExecutor, HttpExecutor};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};

use app::{Action, Command, Outcome, State};

#[derive(Parser)]
#[command(version, about)]
struct Cli {}

fn main() -> Result<()> {
    Cli::parse();

    let executor: Arc<dyn HttpExecutor> = Arc::new(CurlExecutor::default());
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
                self.cancel = Some(cancel.clone());

                let executor = Arc::clone(&self.executor);
                let results = self.results.clone();

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
