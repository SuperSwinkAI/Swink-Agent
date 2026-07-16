//! Cost carried on the `AgentEvent::MessageEnd` event (issues #1100, #1084).
//!
//! Event consumers — the TUI status bar and `/usage`, any `EventForwarder` —
//! read cost from `MessageEnd`, not from the loop's internal accumulator. That
//! makes this a distinct contract from the budget-policy coverage in
//! `policies/tests/policy_integration.rs`: the loop could accumulate correct
//! cost while every observer still displayed `$0.0000`, which is exactly what
//! happened when pricing lived only at the turn level.

use std::sync::Arc;

use futures::StreamExt;
use swink_agent::testing::{MockStreamFn, user_msg};
use swink_agent::{
    Agent, AgentEvent, AgentOptions, AssistantMessageEvent, Cost, ModelRates, ModelSpec,
    PricingTable, StopReason, StreamFn, Usage,
};

/// A scripted response with real usage and zero cost — what every built-in
/// remote adapter except the proxy actually emits.
fn unpriced_response(input: u64) -> Vec<AssistantMessageEvent> {
    vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            usage: Usage::default().with_input(input),
            cost: Cost::default(),
        },
    ]
}

/// Run one prompt and return the cost carried on the `MessageEnd` event.
async fn message_end_cost(
    model_id: &str,
    configure: impl FnOnce(AgentOptions) -> AgentOptions,
) -> f64 {
    let stream_fn: Arc<dyn StreamFn> =
        Arc::new(MockStreamFn::new(vec![unpriced_response(1_000_000)]));
    let options = AgentOptions::new_simple("system", ModelSpec::new("test", model_id), stream_fn);
    let mut agent = Agent::new(configure(options));

    let stream = agent
        .prompt_stream(vec![user_msg("go")])
        .expect("agent should start");
    let mut stream = std::pin::pin!(stream);

    let mut cost = None;
    while let Some(event) = stream.next().await {
        if let AgentEvent::MessageEnd { message } = event {
            cost = Some(message.cost.total);
        }
    }
    cost.expect("loop should emit MessageEnd")
}

/// `MessageEnd` must carry catalog pricing. Before this, `MessageEnd` was
/// emitted from the streaming layer while pricing happened later in the turn,
/// so observers saw zero.
#[tokio::test]
async fn message_end_carries_catalog_pricing() {
    let cost = message_end_cost("claude-sonnet-4-6", |options| options).await;
    assert!(
        (cost - 3.0).abs() < 1e-9,
        "MessageEnd should carry catalog pricing ($3.00/M input), got ${cost:.4}"
    );
}

/// Operator-declared rates must reach `MessageEnd` too, and outrank the catalog.
#[tokio::test]
async fn message_end_carries_operator_declared_pricing() {
    let table = PricingTable::new().with_model(
        "claude-sonnet-4-6",
        ModelRates::default().with_input_per_million(1.0),
    );
    let cost = message_end_cost("claude-sonnet-4-6", |options| {
        options.with_pricing_table(table)
    })
    .await;
    assert!(
        (cost - 1.0).abs() < 1e-9,
        "operator rate ($1.00/M) should win over the catalog's $3.00/M, got ${cost:.4}"
    );
}

/// A closure calculator is the escape hatch for tiered or per-tenant rates.
#[tokio::test]
async fn message_end_carries_closure_supplied_pricing() {
    let cost = message_end_cost("my-local-llama", |options| {
        options.with_cost_calculator(|model_id: &str, _usage: &Usage| {
            (model_id == "my-local-llama").then(|| Cost::default().with_total(0.75))
        })
    })
    .await;
    assert!((cost - 0.75).abs() < 1e-9, "got ${cost:.4}");
}

/// A model with neither catalog nor declared pricing must report an honest zero.
#[tokio::test]
async fn message_end_reports_zero_for_an_unpriced_model() {
    let cost = message_end_cost("totally-unknown-model", |options| options).await;
    assert!((cost).abs() < 1e-9, "got ${cost:.4}");
}
