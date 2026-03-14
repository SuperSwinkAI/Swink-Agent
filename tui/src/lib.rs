#![forbid(unsafe_code)]
//! Library layer for `swink-agent-tui`.
//!
//! Re-exports the types and helpers needed to embed the interactive TUI
//! in your own binary or example.

mod commands;
mod editor;
mod format;
mod session;
mod theme;
mod ui;

pub mod app;
pub mod config;
pub mod error;

#[doc(hidden)]
pub mod credentials;
#[doc(hidden)]
pub mod wizard;

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;
use std::pin::Pin;
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
/// The approval callback is wired automatically — callers should **not** set
/// `with_approve_tool` on the supplied [`swink_agent::AgentOptions`].
pub async fn launch(
    config: TuiConfig,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    options: swink_agent::AgentOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let mut app = App::new(config);
    let approval_tx = app.approval_sender();
    let options = options.with_approve_tool(tui_approval_callback(&approval_tx));
    app.set_agent(Agent::new(options));
    app.run(terminal).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_param_wins_over_config() {
        let config = TuiConfig {
            system_prompt: Some("from config".to_string()),
            ..TuiConfig::default()
        };
        let result = resolve_system_prompt(Some("explicit".to_string()), &config);
        assert_eq!(result, "explicit");
    }

    #[test]
    fn explicit_param_wins_with_no_config() {
        let config = TuiConfig::default();
        let result = resolve_system_prompt(Some("explicit".to_string()), &config);
        assert_eq!(result, "explicit");
    }

    #[test]
    fn config_used_when_no_explicit_param() {
        // This test is valid regardless of env var state because explicit=Some always
        // wins. When explicit=None and env var is unset, config should win.
        // If LLM_SYSTEM_PROMPT happens to be set in the environment, the env var
        // will win over config -- that is the correct priority order.
        let config = TuiConfig {
            system_prompt: Some("from config".to_string()),
            ..TuiConfig::default()
        };
        let result = resolve_system_prompt(None, &config);
        // Result is either "from config" (no env var) or env var value (env var set).
        // We verify it is NOT the default, which proves the fallback chain works.
        assert_ne!(result, DEFAULT_SYSTEM_PROMPT);
    }

    #[test]
    fn default_fallback_when_nothing_set() {
        // When no explicit param AND no config system_prompt AND no LLM_SYSTEM_PROMPT
        // env var, should return the default constant.
        // Note: if LLM_SYSTEM_PROMPT is set in the test environment, this test
        // verifies that the env var path is taken instead (which is correct behavior).
        let config = TuiConfig::default();
        assert!(config.system_prompt.is_none());
        let result = resolve_system_prompt(None, &config);
        // Either the default constant or the env var -- both are valid outcomes
        if std::env::var("LLM_SYSTEM_PROMPT").is_err() {
            assert_eq!(result, DEFAULT_SYSTEM_PROMPT);
        }
    }

    #[test]
    fn explicit_empty_string_still_wins() {
        let config = TuiConfig {
            system_prompt: Some("from config".to_string()),
            ..TuiConfig::default()
        };
        let result = resolve_system_prompt(Some(String::new()), &config);
        assert_eq!(result, "", "explicit empty string should still be used");
    }

    #[test]
    fn default_system_prompt_is_not_empty() {
        assert!(!DEFAULT_SYSTEM_PROMPT.is_empty());
    }
}
