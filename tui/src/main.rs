//! Agent Harness TUI — interactive terminal interface for LLM agents.

mod app;
mod commands;
mod config;
mod credentials;
mod format;
mod session;
mod theme;
mod ui;
mod wizard;

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use agent_harness::{
    Agent, AgentMessage, AgentOptions, ModelSpec, ProxyStreamFn, StreamFn, ToolApproval,
    ToolApprovalRequest, selective_approve,
};
use agent_harness_adapters::{AnthropicStreamFn, OllamaStreamFn, OpenAiStreamFn};
use tokio::sync::{mpsc, oneshot};

use crate::app::App;
use crate::config::TuiConfig;

type AppResult<T> = Result<T, Box<dyn std::error::Error>>;

/// Default system prompt used when no explicit prompt, env var, or config is provided.
const DEFAULT_SYSTEM_PROMPT: &str = "You are a helpful assistant.";

fn main() -> AppResult<()> {
    dotenvy::dotenv().ok();

    // Initialize file-based tracing (TUI owns stdout, so we log to a file).
    let log_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("agent-harness")
        .join("logs");
    let file_appender = tracing_appender::rolling::daily(log_dir, "agent-harness.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("agent_harness=info".parse().unwrap()),
        )
        .with_writer(non_blocking)
        .with_ansi(false)
        .init();

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
    // Run setup wizard on first launch if no API keys are configured
    if !credentials::any_key_configured() {
        let mut wiz = wizard::SetupWizard::new();
        if !wiz.run(&mut terminal)? {
            return Ok(()); // User chose to quit from wizard
        }
    }

    let config = TuiConfig::load();
    let rt = tokio::runtime::Runtime::new()?;

    rt.block_on(async {
        let system_prompt = resolve_system_prompt(None, &config);
        let mut app = App::new(config);
        let approval_tx = app.approval_sender();

        app.set_agent(create_agent(system_prompt, &approval_tx));

        app.run(&mut terminal).await
    })
}

/// Resolve the system prompt from multiple sources.
///
/// Priority: explicit parameter > `LLM_SYSTEM_PROMPT` env var > config file > default constant.
fn resolve_system_prompt(explicit: Option<String>, config: &TuiConfig) -> String {
    explicit
        .or_else(|| std::env::var("LLM_SYSTEM_PROMPT").ok())
        .or_else(|| config.system_prompt.clone())
        .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string())
}

/// Create an agent from environment variables.
///
/// Supports four providers (checked in priority order):
///
/// **Proxy (custom SSE endpoint) — highest priority:**
/// - `LLM_BASE_URL` — proxy endpoint (takes priority if set)
/// - `LLM_API_KEY` — bearer token
/// - `LLM_MODEL` — model identifier
///
/// **OpenAI (or any OpenAI-compatible API):**
/// - `OPENAI_API_KEY` — API key (env var or keychain)
/// - `OPENAI_BASE_URL` — API base URL (default: `https://api.openai.com`)
/// - `OPENAI_MODEL` — model name (default: `gpt-4o`)
///
/// **Anthropic (native Claude API):**
/// - `ANTHROPIC_API_KEY` — API key (env var or keychain)
/// - `ANTHROPIC_BASE_URL` — API base URL (default: `https://api.anthropic.com`)
/// - `ANTHROPIC_MODEL` — model name (default: `claude-sonnet-4-20250514`)
///
/// **Ollama (default) — lowest priority:**
/// - `OLLAMA_HOST` — Ollama server URL (default: `http://localhost:11434`)
/// - `OLLAMA_MODEL` — model name (default: `llama3.2`)
#[allow(clippy::doc_markdown)] // "OpenAI" is a proper noun, not code.
fn create_agent(
    system_prompt: String,
    approval_tx: &mpsc::Sender<(ToolApprovalRequest, oneshot::Sender<ToolApproval>)>,
) -> Agent {

    // Check for proxy mode first (highest priority)
    if let Ok(base_url) = std::env::var("LLM_BASE_URL") {
        let proxy_provider = credentials::providers()
            .into_iter()
            .find(|p| p.key_name == "proxy");
        let api_key = proxy_provider
            .as_ref()
            .and_then(credentials::credential)
            .unwrap_or_default();
        let model_id =
            std::env::var("LLM_MODEL").unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string());
        let proxy: Arc<dyn StreamFn> = Arc::new(ProxyStreamFn::new(&base_url, &api_key));
        let model = ModelSpec::new("proxy", &model_id);
        return build_agent(system_prompt, model, proxy, approval_tx);
    }

    // Check for OpenAI (second priority)
    let openai_provider = credentials::providers()
        .into_iter()
        .find(|p| p.key_name == "openai");
    let openai_key = openai_provider.as_ref().and_then(credentials::credential);
    if let Some(api_key) = openai_key {
        let base_url = std::env::var("OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com".to_string());
        let model_id = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".to_string());
        let openai: Arc<dyn StreamFn> = Arc::new(OpenAiStreamFn::new(&base_url, &api_key));
        let model = ModelSpec::new("openai", &model_id);
        return build_agent(system_prompt, model, openai, approval_tx);
    }

    // Check for Anthropic (third priority)
    let anthropic_provider = credentials::providers()
        .into_iter()
        .find(|p| p.key_name == "anthropic");
    let anthropic_key = anthropic_provider
        .as_ref()
        .and_then(credentials::credential);
    if let Some(api_key) = anthropic_key {
        let base_url = std::env::var("ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com".to_string());
        let model_id = std::env::var("ANTHROPIC_MODEL")
            .unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string());
        let anthropic: Arc<dyn StreamFn> =
            Arc::new(AnthropicStreamFn::new(&base_url, &api_key));
        let model = ModelSpec::new("anthropic", &model_id);
        return build_agent(system_prompt, model, anthropic, approval_tx);
    }

    // Default: Ollama (lowest priority)
    let host =
        std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".to_string());
    let model_id = std::env::var("OLLAMA_MODEL").unwrap_or_else(|_| "llama3.2".to_string());
    let ollama: Arc<dyn StreamFn> = Arc::new(OllamaStreamFn::new(&host));
    let model = ModelSpec::new("ollama", &model_id);
    build_agent(system_prompt, model, ollama, approval_tx)
}

fn build_agent(
    system_prompt: String,
    model: ModelSpec,
    stream_fn: Arc<dyn StreamFn>,
    approval_tx: &mpsc::Sender<(ToolApprovalRequest, oneshot::Sender<ToolApproval>)>,
) -> Agent {
    let tx = approval_tx.clone();
    let approve_callback = selective_approve(move |request: ToolApprovalRequest| {
        let tx = tx.clone();
        Box::pin(async move {
            let (resp_tx, resp_rx) = oneshot::channel();
            if tx.send((request, resp_tx)).await.is_err() {
                // Channel closed — auto-approve to avoid blocking the agent
                return ToolApproval::Approved;
            }
            resp_rx.await.unwrap_or(ToolApproval::Approved)
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = ToolApproval> + Send>>
    });

    Agent::new(
        AgentOptions::new(system_prompt, model, stream_fn, |msg: &AgentMessage| match msg {
            AgentMessage::Llm(llm) => Some(llm.clone()),
            AgentMessage::Custom(_) => None,
        })
        .with_approve_tool(approve_callback),
    )
}
