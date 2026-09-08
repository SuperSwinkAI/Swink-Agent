//! `swink-agentd` — JSON-RPC agent daemon.
//!
//! Hosts a `swink-agent` Agent behind a Unix socket. Requires the `cli` feature.

#[cfg(unix)]
mod unix_main {
    use clap::Parser;
    use swink_agent::{AgentOptions, DEFAULT_MODEL, DEFAULT_SYSTEM_PROMPT, ModelConnections};
    use swink_agent_adapters::build_remote_connection_for_model;
    use swink_agent_rpc::AgentServer;
    use tracing_subscriber::EnvFilter;

    #[derive(Debug, Parser)]
    #[command(name = "swink-agentd", about = "JSON-RPC agent daemon for swink-agent")]
    struct Cli {
        /// Unix socket path to listen on.
        #[arg(long, short = 'l', default_value = "/tmp/swink.sock")]
        listen: std::path::PathBuf,

        /// Remove an existing socket file before binding.
        #[arg(long, default_value_t = false)]
        force: bool,

        /// Model to use (e.g. claude-sonnet-4-6, gpt-4.1).
        ///
        /// Falls back to the `LLM_MODEL` environment variable (also loaded from
        /// `.env`), then to the built-in default (claude-sonnet-4-6).
        #[arg(long, short = 'm')]
        model: Option<String>,

        /// System prompt.
        ///
        /// Falls back to the `LLM_SYSTEM_PROMPT` environment variable (also
        /// loaded from `.env`), then to the built-in default prompt.
        #[arg(long, short = 's')]
        system_prompt: Option<String>,
    }

    /// Resolve a config value through the standard fallback chain.
    ///
    /// Priority: explicit CLI flag > environment variable > default constant.
    /// This mirrors the TUI's `resolve_system_prompt` pattern so both binaries
    /// honor `LLM_MODEL` / `LLM_SYSTEM_PROMPT` identically. Environment access
    /// stays in [`run`]; this function is pure so it can be tested without
    /// touching process-global state.
    fn resolve(cli_flag: Option<String>, env_var: Option<String>, default: &str) -> String {
        cli_flag.or(env_var).unwrap_or_else(|| default.to_string())
    }

    pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .init();

        dotenvy::dotenv().ok();

        let cli = Cli::parse();

        let model = resolve(cli.model, std::env::var("LLM_MODEL").ok(), DEFAULT_MODEL);
        let system_prompt = resolve(
            cli.system_prompt,
            std::env::var("LLM_SYSTEM_PROMPT").ok(),
            DEFAULT_SYSTEM_PROMPT,
        );

        let factory = move || -> Result<AgentOptions, String> {
            let connection = build_remote_connection_for_model(&model)
                .map_err(|e| format!("failed to build model connection: {e}"))?;
            let connections = ModelConnections::builder().primary(connection).build();
            Ok(AgentOptions::from_connections(&system_prompt, connections).with_default_tools())
        };

        let server = if cli.force {
            AgentServer::bind_force(&cli.listen, factory)
        } else {
            AgentServer::bind(&cli.listen, factory)?
        };

        server.serve().await?;
        Ok(())
    }

    #[cfg(test)]
    #[path = "tests.rs"]
    mod tests;
}

#[cfg(unix)]
#[tokio::main]
async fn main() {
    if let Err(e) = unix_main::run().await {
        eprintln!("swink-agentd: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("swink-agentd: Unix socket transport requires a Unix host.");
    eprintln!("On Windows, use the in-process TUI instead.");
    std::process::exit(1);
}
