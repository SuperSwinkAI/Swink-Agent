//! End-to-end example: derive a complete `AgentTool` with `#[tool]` and a
//! parameter schema with `#[derive(ToolSchema)]`, then execute the generated
//! tool against the real `swink-agent` trait.
//!
//! This is the compile-checked reference for the crate's intended audience:
//! **external SDK consumers** writing function-style tools. The built-in
//! tools inside the `swink-agent` crate itself intentionally hand-roll
//! `AgentTool` — they need constructor state, custom display labels,
//! `deny_unknown_fields` schemas, and `execution_root`/`approval_context`
//! hooks that the macro deliberately does not model. See the crate README
//! for the full scope statement.
//!
//! Run with: `cargo run -p swink-agent-macros --example derived_tool`

// `#[tool]` emits `pub struct EchoTool;`. In this example binary it is not
// exported API, so the semver `#[non_exhaustive]` guard does not apply.
#![allow(clippy::exhaustive_structs)]

use std::sync::{Arc, RwLock};

use swink_agent::{
    AgentTool, AgentToolResult, ContentBlock, IntoTool, JsonSchema, SessionState, ToolParameters,
};
use swink_agent_macros::{ToolSchema, tool};
use tokio_util::sync::CancellationToken;

// ─── #[tool]: async fn → AgentTool ──────────────────────────────────────────

// Expands to a unit struct `EchoTool` implementing `swink_agent::AgentTool`.
// The JSON Schema is derived from the non-token parameters via `schemars`;
// the `CancellationToken` parameter is excluded from the schema and receives
// the token that the agent loop passes to `execute`.
#[tool(name = "echo", description = "Echo a message, optionally repeated")]
async fn echo(message: String, times: Option<u32>, cancel: CancellationToken) -> AgentToolResult {
    if cancel.is_cancelled() {
        return AgentToolResult::error("cancelled");
    }
    let repeated: Vec<&str> = (0..times.unwrap_or(1)).map(|_| message.as_str()).collect();
    AgentToolResult::text(repeated.join(" "))
}

// ─── #[derive(ToolSchema)]: struct → ToolParameters ─────────────────────────

/// `#[derive(ToolSchema)]` implements `ToolParameters` by delegating schema
/// generation to `schemars`. Doc comments become field descriptions.
#[derive(ToolSchema, JsonSchema)]
#[allow(dead_code)] // schema-only in this example; execution uses `#[tool]` above
struct SearchParams {
    /// The search query.
    query: String,
    /// Maximum number of results.
    limit: Option<u32>,
}

#[tokio::main]
async fn main() {
    // The generated tool is a plain value implementing the real trait, so it
    // slots into a tool registry exactly like a hand-rolled implementation.
    let tool: Arc<dyn AgentTool> = EchoTool.into_tool();

    println!("name:        {}", tool.name());
    println!("description: {}", tool.description());
    println!("schema:      {}", tool.parameters_schema());

    let result = tool
        .execute(
            "call-1",
            serde_json::json!({ "message": "hello", "times": 3 }),
            CancellationToken::new(),
            None,
            Arc::new(RwLock::new(SessionState::new())),
            None,
        )
        .await;

    assert!(!result.is_error, "echo tool must succeed on valid params");
    println!(
        "result:      {}",
        ContentBlock::extract_text(&result.content)
    );

    // Standalone parameter schema via #[derive(ToolSchema)]. UFCS because
    // `schemars::JsonSchema` also exposes a `json_schema` associated fn.
    println!(
        "SearchParams schema: {}",
        <SearchParams as ToolParameters>::json_schema()
    );
}
