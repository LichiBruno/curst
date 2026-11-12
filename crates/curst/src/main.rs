//! curst: a terminal API client built on libcurl

#![cfg_attr(test, allow(clippy::panic))]

mod app;

use anyhow::{Context, Result};
use clap::Parser;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};

#[derive(Parser)]
#[command(version, about)]
struct Cli {}

fn main() -> Result<()> {
    Cli::parse();
    let mut terminal = ratatui::try_init().context("could not start the terminal UI")?;
    let result = run(&mut terminal);
    ratatui::try_restore().context("could not restore the terminal")?;
    result
}

fn run(terminal: &mut DefaultTerminal) -> Result<()> {
    let mut state = app::State::default();
    while !state.should_quit {
        terminal.draw(|frame| app::view(&state, frame))?;
        if let Event::Key(key) = event::read()?
            && let Some(action) = app::action_for(key)
        {
            app::update(&mut state, action);
        }
    }
    Ok(())
}
