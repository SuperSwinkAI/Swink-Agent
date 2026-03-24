//! Example: configure policy slots on an Agent.
//!
//! Demonstrates using built-in policies to enforce budget limits, restrict
//! tool access, cap turn count, and detect stuck loops — all via the
//! composable policy slot system. No policies are enabled by default;
//! you opt in to exactly the guardrails you need.

use std::pin::Pin;
use std::sync::{Arc, Mutex};

use futures::Stream;
use tokio_util::sync::CancellationToken;

use swink_agent::prelude::*;
use swink_agent::{BudgetPolicy, MaxTurnsPolicy, ToolDenyListPolicy};

// ─── Mock StreamFn ──────────────────────────────────────────────────────────

/// A mock `StreamFn` that returns canned text responses.
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
                    error_kind: None,
                }]
            } else {
                responses.remove(0)
            }
        };
        Box::pin(futures::stream::iter(events))
    }
}

fn text_events(text: &str) -> Vec<AssistantMessageEvent> {
    AssistantMessageEvent::text_response(text)
}

// ─── Main ───────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        text_events("Turn 1 response"),
        text_events("Turn 2 response"),
        text_events("Turn 3 response"),
        text_events("Turn 4 — this should not appear"),
    ]));

    let model = ModelSpec::new("mock", "mock-model-v1");

    // ── Build an agent with policy guardrails ──

    let options = AgentOptions::new_simple("You are a helpful assistant.", model, stream_fn)
        // PreTurn policies: checked before each LLM call.
        // Budget: stop if cost exceeds $10.
        .with_pre_turn_policy(BudgetPolicy::new().max_cost(10.0))
        // Max turns: stop after 3 turns.
        .with_pre_turn_policy(MaxTurnsPolicy::new(3))
        // PreDispatch policies: checked per tool call, before approval.
        // Deny list: block "bash" tool calls entirely.
        .with_pre_dispatch_policy(ToolDenyListPolicy::new(["bash"]));

    // Policies are evaluated in the order they were added.
    // BudgetPolicy is checked first; if it passes, MaxTurnsPolicy is checked.
    // If either returns Stop, the loop halts before calling the LLM.

    let mut agent = Agent::new(options);

    // ── Run the agent with follow-ups to demonstrate turn limiting ──

    let result = agent
        .prompt_text("Hello!")
        .await
        .expect("prompt failed");
    println!("Turn 1: {}", result.assistant_text());

    let result = agent.continue_async().await.expect("continue failed");
    println!("Turn 2: {}", result.assistant_text());

    let result = agent.continue_async().await.expect("continue failed");
    println!("Turn 3: {}", result.assistant_text());

    // The 4th turn would be blocked by MaxTurnsPolicy (max_turns=3).
    // The agent will stop before making the LLM call.
    let result = agent.continue_async().await;
    match result {
        Ok(r) => println!("Turn 4: {}", r.assistant_text()),
        Err(e) => println!("Turn 4 blocked: {e}"),
    }

    println!("\nAgent stopped after {} turns (policy enforced).", 3);
}
