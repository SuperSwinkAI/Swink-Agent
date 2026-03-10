//! Agent Harness TUI — interactive terminal interface for LLM agents.

mod app;
mod event;
mod theme;
mod ui;

use std::io;

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::app::App;

type AppResult<T> = Result<T, Box<dyn std::error::Error>>;

fn main() -> AppResult<()> {
    // Install panic hook that restores terminal before printing panic.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        original_hook(info);
    }));

    let terminal = setup_terminal()?;
    let result = run(terminal);
    restore_terminal()?;
    result
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend)
}

fn restore_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    Ok(())
}

fn run(mut terminal: Terminal<CrosstermBackend<io::Stdout>>) -> AppResult<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async { App::new().run(&mut terminal).await })
}
