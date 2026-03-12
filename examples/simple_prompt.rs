//! Minimal example: create an Agent with a mock stream function, send a prompt,
//! and print the result.
//!
//! This demonstrates the core API without any real LLM provider. In production
//! you would replace `MockStreamFn` with an adapter from `swink-agent-adapters`
//! (e.g. `AnthropicStreamFn`, `OpenAiStreamFn`, `OllamaStreamFn`).

use std::pin::Pin;
use std::sync::Mutex;

use futures::Stream;
use tokio_util::sync::CancellationToken;

use swink_agent::{
    Agent, AgentMessage, AgentOptions, AssistantMessageEvent, ContentBlock, Cost, LlmMessage,
    ModelSpec, StopReason, StreamFn, StreamOptions, Usage, default_convert,
};

// ─── Mock StreamFn ──────────────────────────────────────────────────────────

/// A mock `StreamFn` that returns a canned text response.
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

/// Build a sequence of events that produces a single text response.
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

// ─── Main ───────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // Step 1: Create a mock stream function with a canned response.
    let stream_fn = std::sync::Arc::new(MockStreamFn::new(vec![text_events(
        "Hello! I'm a mock LLM response.",
    )]));

    // Step 2: Define the model specification.
    let model = ModelSpec::new("mock", "mock-model-v1");

    // Step 3: Build agent options with defaults.
    let options = AgentOptions::new(
        "You are a helpful assistant.",
        model,
        stream_fn,
        default_convert,
    );

    // Step 4: Create the agent.
    let mut agent = Agent::new(options);

    // Step 5: Send a prompt and await the result.
    let result = agent
        .prompt_text("What is Rust?")
        .await
        .expect("prompt failed");

    // Step 6: Extract and print the response text.
    for msg in &result.messages {
        if let AgentMessage::Llm(LlmMessage::Assistant(assistant)) = msg {
            let text = ContentBlock::extract_text(&assistant.content);
            println!("Assistant: {text}");
        }
    }

    println!("Stop reason: {:?}", result.stop_reason);
    println!("Usage: {:?}", result.usage);
}
