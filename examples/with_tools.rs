//! Example: register tools with an Agent and set up the approval callback.
//!
//! Demonstrates how to wire up `BashTool` and `ReadFileTool`, configure an
//! approval callback via `with_approve_tool_async`, and run a prompt.
//!
//! Also shows how to create a custom tool with [`FnTool`] using closures
//! instead of implementing the [`AgentTool`] trait manually.

use std::pin::Pin;
use std::sync::{Arc, Mutex};

use futures::Stream;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use swink_agent::prelude::*;
use swink_agent::{JsonSchema, ToolApproval};

// ─── Mock StreamFn ──────────────────────────────────────────────────────────

/// A mock `StreamFn` that yields scripted event sequences.
struct MockStreamFn {
    responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
}

impl MockStreamFn {
    const fn new(responses: Vec<Vec<AssistantMessageEvent>>) -> Self {
        Self {
            responses: Mutex::new(responses),
        }
    }
}

impl StreamFn for MockStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        _context: &'a swink_agent::AgentContext,
        _options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        let events = {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                vec![AssistantMessageEvent::Error {
                    stop_reason: StopReason::Error,
                    error_message: "no more scripted responses".to_string(),
                    error_kind: None,
                    usage: None,
                }]
            } else {
                responses.remove(0)
            }
        };
        Box::pin(futures::stream::iter(events))
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn text_events(text: &str) -> Vec<AssistantMessageEvent> {
    AssistantMessageEvent::text_response(text)
}

// ─── Custom tool params ─────────────────────────────────────────────────────

/// Parameters for the `get_weather` custom tool.
#[derive(Deserialize, JsonSchema)]
#[allow(dead_code)]
struct GetWeatherParams {
    /// The city to look up weather for (e.g. "San Francisco").
    city: String,
}

// ─── Main ───────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // Step 1a: Create built-in tools.
    // `builtin_tools()` returns Vec<Arc<dyn AgentTool>> with BashTool, ReadFileTool, WriteFileTool.
    let mut tools = builtin_tools();

    // Step 1b: Create a custom tool using `FnTool` — no need to implement `AgentTool` manually.
    let weather = FnTool::new(
        "get_weather",
        "Weather",
        "Get the current weather for a city.",
    )
    .with_schema_for::<GetWeatherParams>()
    .with_execute_simple(|params, _cancel| async move {
        let city = params["city"].as_str().unwrap_or("unknown");
        // In a real application this would call a weather API.
        AgentToolResult::text(format!("72°F and sunny in {city}"))
    })
    .into_tool();

    tools.push(weather);

    // Step 2: Set up a mock stream function (replace with a real adapter).
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_events(
        "I would use the bash tool to list files, but this is a mock.",
    )]));

    let model = ModelSpec::new("mock", "mock-model-v1");

    // Step 3: Build options with tools and an approval callback.
    //
    // `with_approve_tool_async` registers an async approval callback that is
    // invoked for every tool call. It avoids the verbose `Pin<Box<dyn Future<...>>>`
    // ceremony that `with_approve_tool` requires.
    //
    // To only prompt for tools that declare `requires_approval() == true`
    // (e.g. BashTool and WriteFileTool, but not ReadFileTool), wrap the
    // callback with `selective_approve` instead:
    //
    //   .with_approve_tool(selective_approve(|req| Box::pin(async move { ... })))
    let options = AgentOptions::new_simple(
        "You are a helpful coding assistant with access to shell and file tools.",
        model,
        stream_fn,
    )
    .with_tools(tools)
    .with_approve_tool_async(|req| async move {
        // In a real application you would prompt the user here.
        println!(
            "Approval requested for tool '{}' with args: {}",
            req.tool_name, req.arguments
        );
        // Auto-approve for this example.
        ToolApproval::Approved
    });

    // Step 4: Create the agent and run a prompt.
    let mut agent = Agent::new(options);

    let result = agent
        .prompt_text("List the files in the current directory.")
        .await
        .expect("prompt failed");

    // Step 5: Print the response.
    println!("Assistant: {}", result.assistant_text());
}
