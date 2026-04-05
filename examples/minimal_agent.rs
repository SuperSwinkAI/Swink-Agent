//! Minimal real-provider agent — ~30 lines, no TUI.
//!
//! Bridges the gap between `simple_prompt.rs` (mock) and `custom_agent.rs`
//! (full TUI + multi-provider). Uses a single real adapter and `prompt_text()`
//! to send one request and print the response.
//!
//! Run: `cargo run --example minimal_agent`
//! Requires: `ANTHROPIC_API_KEY` (or swap the preset for OpenAI/Ollama).

use swink_agent::{Agent, AgentMessage, AgentOptions, ContentBlock, LlmMessage, ModelConnections};
use swink_agent_adapters::build_remote_connection_for_model;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    // Build a single remote connection by model_id (reads the API key from env).
    let connection = build_remote_connection_for_model("claude-haiku-4-5-20251001")?;

    // Create agent options with just the primary model — no fallbacks, no tools.
    let connections = ModelConnections::new(connection, vec![]);
    let options = AgentOptions::from_connections("You are a helpful assistant.", connections);

    // Create and prompt.
    let mut agent = Agent::new(options);
    let result = agent.prompt_text("What is Rust?").await?;

    // Print the response.
    for msg in &result.messages {
        if let AgentMessage::Llm(LlmMessage::Assistant(a)) = msg {
            println!("{}", ContentBlock::extract_text(&a.content));
        }
    }
    Ok(())
}
