//! Build and launch a custom LLM agent with the full interactive TUI.
//!
//! This is the minimal example of importing `swink-agent`, `swink-agent-adapters`,
//! and `swink-agent-tui` as external crates — the same code you'd write in your
//! own project after `cargo add`ing the three crates.
//!
//! Run: `cargo run --example custom_agent`
//! Requires: `ANTHROPIC_API_KEY` in `.env` or environment.

use std::sync::Arc;

use swink_agent::{
    Agent, AgentMessage, AgentOptions, AgentTool, BashTool, ModelSpec, ReadFileTool, StreamFn,
    WriteFileTool,
};
use swink_agent_adapters::AnthropicStreamFn;
use swink_agent_tui::{
    TuiConfig, launch, restore_terminal, setup_terminal, tui_approval_callback,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    // 1. Read API key from environment.
    let api_key =
        std::env::var("ANTHROPIC_API_KEY").expect("set ANTHROPIC_API_KEY in .env or environment");

    // 2. Create the Anthropic streaming adapter.
    let base_url = std::env::var("ANTHROPIC_BASE_URL")
        .unwrap_or_else(|_| "https://api.anthropic.com".to_string());
    let stream_fn: Arc<dyn StreamFn> = Arc::new(AnthropicStreamFn::new(&base_url, &api_key));

    // 3. Choose a model.
    let model = ModelSpec::new("anthropic", "claude-sonnet-4-20250514");

    // 4. Register tools.
    let tools: Vec<Arc<dyn AgentTool>> = vec![
        Arc::new(BashTool::new()),
        Arc::new(ReadFileTool::new()),
        Arc::new(WriteFileTool::new()),
    ];

    // 5. Set up terminal and launch the TUI.
    let mut terminal = setup_terminal()?;

    let result = launch(TuiConfig::default(), &mut terminal, |approval_tx| {
        let options = AgentOptions::new(
            "You are a helpful coding assistant.",
            model,
            stream_fn,
            |msg: &AgentMessage| match msg {
                AgentMessage::Llm(llm) => Some(llm.clone()),
                AgentMessage::Custom(_) => None,
            },
        )
        .with_tools(tools)
        .with_approve_tool(tui_approval_callback(approval_tx));

        Agent::new(options)
    })
    .await;

    restore_terminal()?;
    result
}
