#![forbid(unsafe_code)]
//! Library layer for `swink-agent-tui`.
//!
//! Re-exports the types and helpers needed to embed the interactive TUI
//! in your own binary or example.

mod commands;
mod format;
mod session;
mod theme;
mod ui;

pub mod app;
pub mod config;

#[doc(hidden)]
pub mod credentials;
#[doc(hidden)]
pub mod wizard;

use std::io;
use std::pin::Pin;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::{mpsc, oneshot};

use swink_agent::{Agent, ToolApproval, ToolApprovalRequest, selective_approve};

pub use app::App;
pub use config::TuiConfig;

/// Sender half of the approval channel used by the TUI.
pub type ApprovalSender = mpsc::Sender<(ToolApprovalRequest, oneshot::Sender<ToolApproval>)>;

/// Boxed approval callback returned by [`tui_approval_callback`].
type ApprovalCallbackFn = Box<
    dyn Fn(ToolApprovalRequest) -> Pin<Box<dyn std::future::Future<Output = ToolApproval> + Send>>
        + Send
        + Sync,
>;

/// Default system prompt used when no explicit prompt, env var, or config is provided.
const DEFAULT_SYSTEM_PROMPT: &str = "You are a helpful assistant.";

/// Initialize the terminal for TUI rendering.
///
/// Enables raw mode, enters the alternate screen, and enables mouse capture.
pub fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend)
}

/// Restore the terminal to its original state.
///
/// Disables raw mode, leaves the alternate screen, and disables mouse capture.
pub fn restore_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    Ok(())
}

/// Resolve the system prompt from multiple sources.
///
/// Priority: explicit parameter > `LLM_SYSTEM_PROMPT` env var > config file > default constant.
pub fn resolve_system_prompt(explicit: Option<String>, config: &TuiConfig) -> String {
    explicit
        .or_else(|| std::env::var("LLM_SYSTEM_PROMPT").ok())
        .or_else(|| config.system_prompt.clone())
        .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string())
}

/// Build the standard TUI approval callback.
///
/// Returns an approval closure (wrapped with [`selective_approve`]) that forwards
/// approval requests through the TUI's approval channel. Use this when constructing
/// an [`Agent`] that will be driven by the TUI event loop.
pub fn tui_approval_callback(approval_tx: &ApprovalSender) -> ApprovalCallbackFn {
    let tx = approval_tx.clone();
    selective_approve(move |request: ToolApprovalRequest| {
        let tx = tx.clone();
        Box::pin(async move {
            let (resp_tx, resp_rx) = oneshot::channel();
            if tx.send((request, resp_tx)).await.is_err() {
                return ToolApproval::Approved;
            }
            resp_rx.await.unwrap_or(ToolApproval::Approved)
        }) as Pin<Box<dyn std::future::Future<Output = ToolApproval> + Send>>
    })
}

/// High-level convenience: create an [`App`], wire up an agent, and run the event loop.
///
/// The `agent_factory` closure receives the [`ApprovalSender`] so it can build
/// the [`Agent`] with the correct approval callback before the loop starts.
pub async fn launch<F>(
    config: TuiConfig,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    agent_factory: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce(&ApprovalSender) -> Agent,
{
    let mut app = App::new(config);
    let approval_tx = app.approval_sender();
    app.set_agent(agent_factory(&approval_tx));
    app.run(terminal).await
}
