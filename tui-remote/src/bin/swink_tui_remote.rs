#![forbid(unsafe_code)]
//! `swink-tui-remote` — run the swink-agent TUI against a remote agent.
//!
//! Connects to a `swink-agentd` Unix socket and drives the stock terminal UI
//! through [`RemoteTransport`]. The agent (model, tools, approval policy)
//! is configured entirely on the server side.

use std::io::IsTerminal as _;
use std::path::PathBuf;

use clap::Parser;
use swink_agent_tui::{App, TuiConfig, restore_terminal, setup_terminal};
use swink_agent_tui_remote::RemoteTransport;

/// Terminal UI for a swink-agent served over JSON-RPC.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Path to the swink-agentd Unix socket (e.g. /tmp/swink.sock).
    socket: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    if !std::io::stdout().is_terminal() {
        eprintln!("Error: swink-tui-remote requires an interactive terminal (TTY).");
        std::process::exit(1);
    }

    // File-based tracing — the TUI owns stdout.
    let log_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("swink-agent")
        .join("logs");
    let file_appender = tracing_appender::rolling::daily(log_dir, "swink-tui-remote.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("swink_agent=info".parse()?),
        )
        .with_writer(non_blocking)
        .with_ansi(false)
        .init();

    // Connect before entering raw mode so connection errors print cleanly.
    let transport = RemoteTransport::connect(&args.socket)
        .await
        .map_err(|e| format!("cannot connect to {}: {e}", args.socket.display()))?;

    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        original_hook(info);
    }));

    let mut terminal = setup_terminal()?;
    let mut app = App::new(TuiConfig::load()).with_transport(Box::new(transport));
    let result = app.run(&mut terminal).await;
    restore_terminal()?;
    result
}
