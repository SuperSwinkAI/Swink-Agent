//! Example: implement the `StreamFn` trait for a custom LLM provider.
//!
//! Shows the complete contract: receive model/context/options, return a stream
//! of `AssistantMessageEvent` values following the start/delta/end protocol.
//! The `DummyStreamFn` here returns a canned response; a real implementation
//! would make HTTP calls to an LLM API.

use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use tokio_util::sync::CancellationToken;

use swink_agent::prelude::*;

// ─── DummyStreamFn ──────────────────────────────────────────────────────────

/// A custom `StreamFn` that echoes the user's last message back.
///
/// Demonstrates the minimum viable implementation of the `StreamFn` trait.
struct DummyStreamFn;

impl StreamFn for DummyStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        context: &'a AgentContext,
        _options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        // Extract the last user message text for the echo response.
        let user_text = context
            .messages
            .iter()
            .rev()
            .find_map(|msg| match msg {
                AgentMessage::Llm(LlmMessage::User(u)) => {
                    Some(ContentBlock::extract_text(&u.content))
                }
                _ => None,
            })
            .unwrap_or_else(|| "...".to_string());

        let response = format!("Echo: {user_text}");

        // Build the event sequence following the start/delta/end protocol:
        //   1. Start — opens the stream
        //   2. TextStart — begins a text content block at index 0
        //   3. TextDelta — incremental text fragment(s)
        //   4. TextEnd — closes the text block
        //   5. Done — terminal event with usage/cost
        let events = vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::TextStart { content_index: 0 },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: response,
            },
            AssistantMessageEvent::TextEnd { content_index: 0 },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Stop,
                usage: Usage {
                    input: 10,
                    output: 5,
                    cache_read: 0,
                    cache_write: 0,
                    total: 15,
                    ..Default::default()
                },
                cost: Cost::default(),
            },
        ];

        Box::pin(futures::stream::iter(events))
    }
}

// ─── Main ───────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // Step 1: Instantiate the custom adapter.
    let stream_fn = Arc::new(DummyStreamFn);

    // Step 2: Configure and create the agent.
    let model = ModelSpec::new("dummy", "echo-v1");
    let options = AgentOptions::new_simple("You are an echo bot.", model, stream_fn);
    let mut agent = Agent::new(options);

    // Step 3: Send a prompt.
    let result = agent
        .prompt_text("Hello, world!")
        .await
        .expect("prompt failed");

    // Step 4: Print results.
    println!("Response: {}", result.assistant_text());

    println!("Token usage: {:?}", result.usage);
}
