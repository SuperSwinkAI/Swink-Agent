//! Build and launch a custom LLM agent with the full interactive TUI.
//!
//! This is the minimal example of importing `swink-agent`, `swink-agent-adapters`,
//! and `swink-agent-tui` as external crates — the same code you'd write in your
//! own project after `cargo add`ing the three crates.
//!
//! Run: `cargo run --example custom_agent`
//! Requires: `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` in `.env` or environment.

use std::sync::Arc;

use swink_agent::{
    Agent, AgentOptions, AgentTool, BashTool, ModelSpec, ReadFileTool, StreamFn, WriteFileTool,
};
use swink_agent_adapters::{AnthropicStreamFn, OpenAiStreamFn};
use swink_agent_local_llm::{LocalModel, LocalStreamFn, ModelConfig};
use swink_agent_tui::{TuiConfig, launch, restore_terminal, setup_terminal, tui_approval_callback};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    // 1. Read API keys from environment.
    let anthropic_api_key =
        std::env::var("ANTHROPIC_API_KEY").expect("set ANTHROPIC_API_KEY in .env or environment");
    let openai_api_key =
        std::env::var("OPENAI_API_KEY").expect("set OPENAI_API_KEY in .env or environment");

    // 2. Create stream functions for the models we want to cycle with F4.
    let anthropic_base_url = std::env::var("ANTHROPIC_BASE_URL")
        .unwrap_or_else(|_| "https://api.anthropic.com".to_string());
    let openai_base_url =
        std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com".to_string());

    let anthropic: Arc<dyn StreamFn> = Arc::new(AnthropicStreamFn::new(
        &anthropic_base_url,
        &anthropic_api_key,
    ));
    let openai: Arc<dyn StreamFn> =
        Arc::new(OpenAiStreamFn::new(&openai_base_url, &openai_api_key));
    let local: Arc<dyn StreamFn> = Arc::new(LocalStreamFn::new(Arc::new(LocalModel::new(
        ModelConfig::default(),
    ))));

    // 3. Set Sonnet 4.6 as the default, with GPT 5.2 and the local model available via F4.
    let model = ModelSpec::new("anthropic", "claude-sonnet-4-6");
    let extra_models = vec![
        (ModelSpec::new("openai", "gpt-5.2"), Arc::clone(&openai)),
        (
            ModelSpec::new("local", "SmolLM3-3B-Q4_K_M"),
            Arc::clone(&local),
        ),
    ];

    // 4. Register tools.
    let tools: Vec<Arc<dyn AgentTool>> = vec![
        Arc::new(BashTool::new()),
        Arc::new(ReadFileTool::new()),
        Arc::new(WriteFileTool::new()),
    ];

    // 5. Set up terminal and launch the TUI.
    let mut terminal = setup_terminal()?;

    let result = launch(TuiConfig::default(), &mut terminal, |approval_tx| {
        let options = AgentOptions::new_simple(
            "You are a helpful coding assistant.",
            model,
            Arc::clone(&anthropic),
        )
        .with_available_models(extra_models)
        .with_tools(tools)
        .with_approve_tool(tui_approval_callback(approval_tx));

        Agent::new(options)
    })
    .await;

    restore_terminal()?;
    result
}
