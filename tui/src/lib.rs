#![forbid(unsafe_code)]
//! Library layer for `swink-agent-tui`.
//!
//! Re-exports the types and helpers needed to embed the interactive TUI
//! in your own binary or example.
//!
//! The supported public API is exposed from the crate root. Internal
//! implementation modules stay private so they do not become stable surface.
//!
//! ```rust
//! use swink_agent_tui::{App, TuiConfig, TuiError};
//! ```
//!
//! ```compile_fail
//! use swink_agent_tui::app::Focus;
//! ```

mod commands;
mod editor;
mod format;
mod mentions;
mod session;
mod skills;
mod theme;
mod ui;

mod app;
mod config;
mod error;
mod extensions;
pub mod transport;

#[cfg(feature = "cli")]
pub mod credentials;
#[cfg(not(feature = "cli"))]
#[allow(dead_code)]
mod credentials;

#[cfg(feature = "cli")]
pub mod wizard;
#[cfg(not(feature = "cli"))]
#[allow(dead_code)]
mod wizard;

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;
use std::pin::Pin;
use tokio::sync::{mpsc, oneshot};
use tracing::warn;

use swink_agent::{Agent, ToolApproval, ToolApprovalRequest};

pub use app::{
    AgentIo, AgentStatus, App, DisplayMessage, EditorState, HunkReview, MessageRole, ModeState,
    OperatingMode, PathCompletion, SessionPersistence, SkillCompletion, TrustFollowUp, TurnUsage,
    UsageTotals, ViewState,
};
pub use config::TuiConfig;
pub use error::TuiError;
pub use extensions::{
    CustomCommandFn, CustomCommandOutcome, MentionResolverFn, PathCandidate, PathCompletionFn,
    SkillCandidate, SkillCompletionFn, SkillDetailsFn, SkillResolverFn, TuiExtensions,
};
pub use mentions::{PathMention, parse_mentions};
pub use session::JsonlSessionStore;
pub use skills::{SkillInvocation, parse_skill_invocation};
pub use swink_agent::{ApprovalMode, ModelRates, PricingTable};
pub use transport::{
    ControlRequest, ControlResponse, InProcessTransport, TransportError, TuiTransport, UserInput,
};
pub use ui::conversation::ConversationView;
pub use ui::diff::{DiffData, Hunk};
pub use ui::input::{InputEditor, MentionQuery};
pub use ui::markdown::markdown_to_lines;
pub use ui::syntax::highlight_code;

/// Sender half of the approval channel used by the TUI.
pub type ApprovalSender = mpsc::Sender<(ToolApprovalRequest, oneshot::Sender<ToolApproval>)>;

/// Boxed approval callback returned by [`tui_approval_callback`].
type ApprovalCallbackFn = Box<
    dyn Fn(ToolApprovalRequest) -> Pin<Box<dyn std::future::Future<Output = ToolApproval> + Send>>
        + Send
        + Sync,
>;

// Default system prompt used when no explicit prompt, env var, or config is
// provided. Shared with `swink-agentd` via the core crate so the binaries
// cannot drift.
use swink_agent::DEFAULT_SYSTEM_PROMPT;

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
/// Returns an approval closure that forwards approval requests through the TUI's
/// approval channel. Use this when constructing an [`Agent`] that will be driven by
/// the TUI event loop.
pub fn tui_approval_callback(approval_tx: &ApprovalSender) -> ApprovalCallbackFn {
    let tx = approval_tx.clone();
    Box::new(move |request: ToolApprovalRequest| {
        let tx = tx.clone();
        Box::pin(async move {
            let (resp_tx, resp_rx) = oneshot::channel();
            if tx.send((request, resp_tx)).await.is_err() {
                warn!("approval channel unavailable; rejecting tool call");
                return ToolApproval::Rejected;
            }
            resp_rx.await.unwrap_or_else(|_| {
                warn!("approval responder dropped; rejecting tool call");
                ToolApproval::Rejected
            })
        }) as Pin<Box<dyn std::future::Future<Output = ToolApproval> + Send>>
    })
}

/// High-level convenience: create an [`App`], wire up an agent, and run the event loop.
///
/// The approval callback is wired automatically — callers should **not** set
/// `with_approve_tool` on the supplied [`swink_agent::AgentOptions`].
///
/// To control the startup approval mode, configure it on `options` before calling:
/// ```ignore
/// let options = AgentOptions::new(...).with_approval_mode(ApprovalMode::Bypassed);
/// launch(config, &mut terminal, options).await?;
/// ```
/// [`App::approval_mode`] reads the mode from the agent, so both sides stay in sync.
/// Any `[pricing]` rates declared in `config` are applied to `options`, so the
/// status line and `/usage` show real money for operator-priced models.
pub async fn launch(
    config: TuiConfig,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    options: swink_agent::AgentOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    launch_with_extensions(config, terminal, options, TuiExtensions::new()).await
}

/// Like [`launch`], but with host-supplied [`TuiExtensions`].
///
/// This is the seam for embedding: anything a host wants to contribute *in
/// code* — today, custom slash commands — is registered on `TuiExtensions`
/// rather than on [`TuiConfig`], which is deserialized from `tui.toml` and so
/// can only hold data.
///
/// # Example
/// ```no_run
/// # use swink_agent::{AgentOptions, ModelSpec, testing::SimpleMockStreamFn};
/// # use swink_agent_tui::{CustomCommandOutcome, TuiConfig, TuiExtensions, launch_with_extensions, setup_terminal};
/// # use std::sync::Arc;
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// # let options = AgentOptions::new_simple("system", ModelSpec::new("mock", "test"), Arc::new(SimpleMockStreamFn::from_text("hi")));
/// let extensions = TuiExtensions::new().with_command("spend", |app, _args| {
///     CustomCommandOutcome::Feedback(format!(
///         "{} turn(s), ${:.4}",
///         app.usage.turn_usage.len(),
///         app.usage.total_cost
///     ))
/// });
///
/// let mut terminal = setup_terminal()?;
/// launch_with_extensions(TuiConfig::load(), &mut terminal, options, extensions).await
/// # }
/// ```
pub async fn launch_with_extensions(
    config: TuiConfig,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    options: swink_agent::AgentOptions,
    extensions: TuiExtensions,
) -> Result<(), Box<dyn std::error::Error>> {
    TuiLauncher::new(config)
        .with_extensions(extensions)
        .launch(terminal, options)
        .await
}

/// Composable launcher: one place that owns the `App` assembly sequence.
///
/// The free-function launchers each accept a fixed combination of seams
/// ([`launch_with_extensions`] takes extensions, [`launch_with_session`]
/// takes a store) and there is no function for every combination — hosts
/// that needed both had to open-code the assembly steps and silently drift
/// from upstream. The builder composes all seams:
///
/// ```no_run
/// # use swink_agent::{AgentOptions, ModelSpec, testing::SimpleMockStreamFn};
/// # use swink_agent_tui::{JsonlSessionStore, TuiConfig, TuiExtensions, TuiLauncher, setup_terminal};
/// # use std::sync::Arc;
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// # let options = AgentOptions::new_simple("system", ModelSpec::new("mock", "test"), Arc::new(SimpleMockStreamFn::from_text("hi")));
/// # let store = JsonlSessionStore::new("/tmp/sessions".into())?;
/// let mut terminal = setup_terminal()?;
/// TuiLauncher::new(TuiConfig::load())
///     .with_extensions(TuiExtensions::new())
///     .with_session_store(store, "tui_chat_1".to_string())
///     .launch(&mut terminal, options)
///     .await
/// # }
/// ```
///
/// [`build`](Self::build) exposes the assembled [`App`] without running the
/// event loop, for hosts that drive the terminal themselves (and for tests).
#[must_use]
pub struct TuiLauncher {
    config: TuiConfig,
    extensions: TuiExtensions,
    session: Option<(JsonlSessionStore, String)>,
    resume_id: Option<String>,
}

impl TuiLauncher {
    /// Start a launcher from a loaded [`TuiConfig`].
    pub fn new(config: TuiConfig) -> Self {
        Self {
            config,
            extensions: TuiExtensions::new(),
            session: None,
            resume_id: None,
        }
    }

    /// Register host-supplied [`TuiExtensions`] (custom commands, skill and
    /// file-mention seams).
    pub fn with_extensions(mut self, extensions: TuiExtensions) -> Self {
        self.extensions = extensions;
        self
    }

    /// Persist turns through `store` under `session_id` instead of the
    /// default `JsonlSessionStore` location.
    pub fn with_session_store(mut self, store: JsonlSessionStore, session_id: String) -> Self {
        self.session = Some((store, session_id));
        self
    }

    /// Load the prior session `resume_id` into the conversation before the
    /// event loop starts. Requires [`with_session_store`](Self::with_session_store);
    /// [`build`](Self::build) fails if the session cannot be loaded.
    pub fn with_resume(mut self, resume_id: String) -> Self {
        self.resume_id = Some(resume_id);
        self
    }

    /// Assemble the [`App`] without running the event loop.
    ///
    /// Performs the canonical launch sequence — dotenv, pricing, extensions,
    /// session store, approval wiring, agent construction, resume — and
    /// returns the ready-to-run `App`. Use this instead of copying the steps
    /// when you need the `App` before (or instead of) [`launch`](Self::launch).
    ///
    /// # Errors
    ///
    /// Returns an error only when a requested resume session cannot be
    /// loaded.
    pub fn build(self, options: swink_agent::AgentOptions) -> io::Result<App> {
        dotenvy::dotenv().ok();
        let options = self.config.apply_pricing(options);
        let mut app = App::new(self.config).with_extensions(self.extensions);
        if let Some((store, session_id)) = self.session {
            app = app.with_session_store(store, session_id);
        }
        let approval_tx = app.approval_sender();
        let options = options.with_approve_tool(tui_approval_callback(&approval_tx));
        app.set_agent(Agent::new(options));
        if let Some(id) = self.resume_id {
            app.resume_into(&id)?;
        }
        Ok(app)
    }

    /// Assemble the [`App`] and run the event loop on `terminal`.
    ///
    /// # Errors
    ///
    /// Returns an error when a requested resume session cannot be loaded or
    /// the event loop fails.
    pub async fn launch(
        self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
        options: swink_agent::AgentOptions,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.build(options)?.run(terminal).await
    }
}

/// Like [`launch`], but with an injectable session store, session ID, and optional resume.
///
/// Use this when embedding the TUI in a host binary that needs to control where sessions
/// are stored (e.g. `~/.superswink/sessions/`) or resume a prior transcript before the
/// event loop starts.
///
/// # Arguments
/// - `store` — session store to use instead of the default `JsonlSessionStore` location.
/// - `session_id` — ID for the new session (e.g. `tui_chat_<uuid-v7>`).
/// - `resume_id` — if `Some(id)`, load that prior session before starting the event loop.
///   Returns [`io::Error`] if the session is not found.
///
/// As with [`launch`], any `[pricing]` rates declared in `config` are applied to
/// `options`. To also register [`TuiExtensions`], use [`TuiLauncher`], which
/// composes every seam.
pub async fn launch_with_session(
    config: TuiConfig,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    options: swink_agent::AgentOptions,
    store: JsonlSessionStore,
    session_id: String,
    resume_id: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut launcher = TuiLauncher::new(config).with_session_store(store, session_id);
    if let Some(id) = resume_id {
        launcher = launcher.with_resume(id.to_string());
    }
    launcher.launch(terminal, options).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn launcher_options() -> swink_agent::AgentOptions {
        use swink_agent::testing::SimpleMockStreamFn;
        swink_agent::AgentOptions::new_simple(
            "system",
            swink_agent::ModelSpec::new("mock", "test"),
            std::sync::Arc::new(SimpleMockStreamFn::from_text("hi")),
        )
    }

    #[tokio::test]
    async fn launcher_build_assembles_agent_extensions_and_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
        let extensions = TuiExtensions::new().with_command("marker", |_app, _args| {
            CustomCommandOutcome::Feedback("ok".into())
        });

        let app = TuiLauncher::new(TuiConfig::default())
            .with_extensions(extensions)
            .with_session_store(store, "launcher-session".to_string())
            .build(launcher_options())
            .unwrap();

        assert!(app.agent_io.agent.is_some(), "agent must be constructed");
        assert_eq!(app.session.session_id, "launcher-session");
        assert!(
            app.extensions.command_names().any(|name| name == "marker"),
            "host command must be registered"
        );
    }

    #[tokio::test]
    async fn launcher_resume_of_missing_session_fails() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

        let result = TuiLauncher::new(TuiConfig::default())
            .with_session_store(store, "fresh".to_string())
            .with_resume("does-not-exist".to_string())
            .build(launcher_options());
        assert!(result.is_err(), "resuming a nonexistent session must fail");
    }

    fn approval_request() -> ToolApprovalRequest {
        ToolApprovalRequest::new(
            "call_1",
            "write_file",
            serde_json::json!({"path": "secret.txt"}),
            true,
        )
    }

    fn read_only_approval_request() -> ToolApprovalRequest {
        ToolApprovalRequest::new(
            "call_read",
            "read_file",
            serde_json::json!({"path": "notes.md"}),
            false,
        )
    }

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

    #[tokio::test]
    async fn approval_callback_rejects_when_channel_send_fails() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);

        let callback = tui_approval_callback(&tx);
        let approval = callback(approval_request()).await;

        assert_eq!(approval, ToolApproval::Rejected);
    }

    #[tokio::test]
    async fn approval_callback_rejects_when_responder_drops() {
        let (tx, mut rx) = mpsc::channel(1);
        let callback = tui_approval_callback(&tx);

        let approval_task = tokio::spawn(async move { callback(approval_request()).await });

        let (_, responder) = rx
            .recv()
            .await
            .expect("approval request should be forwarded");
        drop(responder);

        assert_eq!(approval_task.await.unwrap(), ToolApproval::Rejected);
    }

    #[tokio::test]
    async fn approval_callback_forwards_read_only_requests_to_tui() {
        let (tx, mut rx) = mpsc::channel(1);
        let callback = tui_approval_callback(&tx);

        let approval_task =
            tokio::spawn(async move { callback(read_only_approval_request()).await });

        for _ in 0..10 {
            tokio::task::yield_now().await;
        }

        let (request, responder) = rx
            .try_recv()
            .expect("read-only approval request should be forwarded");
        assert_eq!(request.tool_name, "read_file");
        assert!(!request.requires_approval);

        responder.send(ToolApproval::Approved).unwrap();
        assert_eq!(approval_task.await.unwrap(), ToolApproval::Approved);
    }
}
