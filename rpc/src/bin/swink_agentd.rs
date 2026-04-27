//! `swink-agentd` — JSON-RPC agent daemon.
//!
//! Hosts a `swink-agent` Agent behind a Unix socket. Requires the `cli` feature.

#[cfg(unix)]
mod unix_main {
    use clap::Parser;
    use swink_agent::{AgentOptions, ModelConnections};
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
        #[arg(long, short = 'm', default_value = "claude-sonnet-4-6")]
        model: String,

        /// System prompt.
        #[arg(long, short = 's', default_value = "You are a helpful assistant.")]
        system_prompt: String,
    }

    pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .init();

        dotenvy::dotenv().ok();

        let cli = Cli::parse();

        let model = cli.model.clone();
        let system_prompt = cli.system_prompt.clone();

        let factory = move || -> AgentOptions {
            let connection = build_remote_connection_for_model(&model)
                .expect("failed to build model connection — check your API key env var");
            let connections = ModelConnections::builder().primary(connection).build();
            AgentOptions::from_connections(&system_prompt, connections).with_default_tools()
        };

        let server = if cli.force {
            AgentServer::bind_force(&cli.listen, factory)
        } else {
            AgentServer::bind(&cli.listen, factory)?
        };

        server.serve().await?;
        Ok(())
    }
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
