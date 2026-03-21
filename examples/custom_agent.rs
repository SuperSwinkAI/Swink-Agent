//! Build and launch a custom LLM agent with the full interactive TUI.
//!
//! This is the minimal example of importing `swink-agent`, `swink-agent-adapters`,
//! and `swink-agent-tui` as external crates — the same code you'd write in your
//! own project after `cargo add`ing the three crates.
//!
//! Run: `cargo run --example custom_agent`
//! Requires: remote provider keys in `.env` or environment. The local SmolLM3-3B
//! model is always available and is included in the F4 cycle by default.

use std::sync::Arc;

use swink_agent::{
    AgentOptions, AgentTool, BashTool, ModelConnections, ReadFileTool, WriteFileTool,
};
use swink_agent_adapters::{build_remote_connection, remote_preset_keys};
use swink_agent_local_llm::default_local_connection;
use swink_agent_tui::{TuiConfig, launch, restore_terminal, setup_terminal};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    // 1. Choose named helpers instead of hand-typing provider/model/base-url strings.
    let connections = ModelConnections::new(
        build_remote_connection(remote_preset_keys::anthropic::SONNET_46)?,
        vec![
            build_remote_connection(remote_preset_keys::openai::GPT_4_1)?,
            default_local_connection()?,
        ],
    );

    // 2. Register tools.
    let tools: Vec<Arc<dyn AgentTool>> = vec![
        Arc::new(BashTool::new()),
        Arc::new(ReadFileTool::new()),
        Arc::new(WriteFileTool::new()),
    ];

    // 3. Build options directly from connections — no manual decomposition needed.
    let options = AgentOptions::from_connections("You are a helpful assistant.", connections)
        .with_tools(tools);

    // 4. Set up terminal and launch the TUI.
    let mut terminal = setup_terminal()?;
    let result = launch(TuiConfig::default(), &mut terminal, options).await;

    restore_terminal()?;
    result
}
