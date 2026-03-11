//! Example: register tools with an Agent and set up the approval callback.
//!
//! Demonstrates how to wire up `BashTool` and `ReadFileTool`, configure the
//! `selective_approve` middleware so only tools that declare
//! `requires_approval = true` go through the approval gate, and run a prompt.

use std::pin::Pin;
use std::sync::{Arc, Mutex};

use futures::Stream;
use tokio_util::sync::CancellationToken;

use agent_harness::{
    Agent, AgentMessage, AgentOptions, AssistantMessageEvent, BashTool, ContentBlock, Cost,
    LlmMessage, ModelSpec, ReadFileTool, StopReason, StreamFn, StreamOptions, ToolApproval,
    ToolApprovalRequest, Usage, WriteFileTool,
};

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
        _context: &'a agent_harness::AgentContext,
        _options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        let events = {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                vec![AssistantMessageEvent::Error {
                    stop_reason: StopReason::Error,
                    error_message: "no more scripted responses".to_string(),
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
    vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextStart { content_index: 0 },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: text.to_string(),
        },
        AssistantMessageEvent::TextEnd { content_index: 0 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ]
}

fn default_convert(msg: &AgentMessage) -> Option<LlmMessage> {
    match msg {
        AgentMessage::Llm(llm) => Some(llm.clone()),
        AgentMessage::Custom(_) => None,
    }
}

// ─── Main ───────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // Step 1: Create tools. Each tool implements `AgentTool`.
    let bash = Arc::new(BashTool::new()) as Arc<dyn agent_harness::AgentTool>;
    let read = Arc::new(ReadFileTool::new()) as Arc<dyn agent_harness::AgentTool>;
    let write = Arc::new(WriteFileTool::new()) as Arc<dyn agent_harness::AgentTool>;

    let tools = vec![bash, read, write];

    // Step 2: Set up a mock stream function (replace with a real adapter).
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_events(
        "I would use the bash tool to list files, but this is a mock.",
    )]));

    let model = ModelSpec::new("mock", "mock-model-v1");

    // Step 3: Build options with tools and an approval callback.
    //
    // `selective_approve` wraps the inner callback so that only tools with
    // `requires_approval() == true` are sent through. BashTool and WriteFileTool
    // require approval; ReadFileTool does not.
    let options = AgentOptions::new(
        "You are a helpful coding assistant with access to shell and file tools.",
        model,
        stream_fn,
        default_convert,
    )
    .with_tools(tools)
    .with_approve_tool(agent_harness::selective_approve(
        |req: ToolApprovalRequest| -> Pin<Box<dyn std::future::Future<Output = ToolApproval> + Send>> {
            Box::pin(async move {
                // In a real application you would prompt the user here.
                println!(
                    "Approval requested for tool '{}' with args: {}",
                    req.tool_name, req.arguments
                );
                // Auto-approve for this example.
                ToolApproval::Approved
            })
        },
    ));

    // Step 4: Create the agent and run a prompt.
    let mut agent = Agent::new(options);

    let result = agent
        .prompt_text("List the files in the current directory.")
        .await
        .expect("prompt failed");

    // Step 5: Print the response.
    for msg in &result.messages {
        if let AgentMessage::Llm(LlmMessage::Assistant(assistant)) = msg {
            let text = ContentBlock::extract_text(&assistant.content);
            println!("Assistant: {text}");
        }
    }
}
