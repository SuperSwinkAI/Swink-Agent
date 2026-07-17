//! End-to-end coverage for operator-declared `[pricing]` rates (issue #1084 §1).
//!
//! These tests run the real agent loop against a mock adapter that behaves like
//! every built-in remote adapter does — it reports token `Usage` but leaves
//! `Cost` at zero — and then feed the resulting events into an `App`, exactly as
//! the TUI event loop does. What the status bar would show is therefore what
//! these tests assert on.

use std::sync::Arc;

use futures::StreamExt;
use swink_agent::testing::{MockStreamFn, user_msg};
use swink_agent::{
    Agent, AgentOptions, AssistantMessageEvent, Cost, ModelSpec, StopReason, StreamFn, Usage,
};
use swink_agent_tui::{App, TuiConfig};

/// One scripted response: real usage, zero cost — what a real adapter emits.
fn unpriced_response(input: u64, output: u64) -> Vec<AssistantMessageEvent> {
    vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            usage: Usage::default().with_input(input).with_output(output),
            cost: Cost::default(),
        },
    ]
}

fn agent_for(model_id: &str, options: impl FnOnce(AgentOptions) -> AgentOptions) -> Agent {
    let stream_fn: Arc<dyn StreamFn> =
        Arc::new(MockStreamFn::new(vec![unpriced_response(1_000_000, 0)]));
    let base = AgentOptions::new_simple("system", ModelSpec::new("test", model_id), stream_fn);
    Agent::new(options(base))
}

/// Drive one prompt through the loop and into an `App`, returning the app.
async fn run_turn_into_app(mut agent: Agent, config: TuiConfig) -> App {
    let mut app = App::new(config);
    let stream = agent
        .prompt_stream(vec![user_msg("go")])
        .expect("agent should start");
    let mut stream = std::pin::pin!(stream);
    while let Some(event) = stream.next().await {
        app.handle_agent_event(event);
    }
    app
}

fn config_with_pricing(toml: &str) -> TuiConfig {
    let config = TuiConfig::from_toml(toml);
    assert!(
        !config.pricing.is_empty(),
        "test fixture should declare pricing"
    );
    config
}

/// The headline case: a model the compiled catalog has never heard of. Without
/// operator-declared rates this turn costs $0.0000 forever — that is the whole
/// reason `[pricing]` exists.
#[tokio::test]
async fn operator_declared_pricing_prices_a_model_absent_from_the_catalog() {
    let config = config_with_pricing(
        r#"
            [pricing."my-local-llama"]
            input_per_million = 2.50
        "#,
    );
    let agent = agent_for("my-local-llama", |options| config.apply_pricing(options));

    let app = run_turn_into_app(agent, config).await;

    assert_eq!(app.usage.total_input_tokens, 1_000_000);
    assert!(
        (app.usage.total_cost - 2.50).abs() < 1e-9,
        "expected the operator's $2.50/M rate to reach the status bar, got ${:.4}",
        app.usage.total_cost
    );
    assert_eq!(app.usage.turn_usage.len(), 1);
    assert!((app.usage.turn_usage[0].cost - 2.50).abs() < 1e-9);
}

/// Operator-declared rates must beat the compiled catalog, not merely fill gaps
/// in it. `claude-sonnet-4-6` is in the catalog at $3.00/M input; an operator
/// who declares $1.00/M must see $1.00.
#[tokio::test]
async fn operator_declared_pricing_takes_precedence_over_the_builtin_catalog() {
    let config = config_with_pricing(
        r#"
            [pricing."claude-sonnet-4-6"]
            input_per_million = 1.00
        "#,
    );
    let agent = agent_for("claude-sonnet-4-6", |options| config.apply_pricing(options));

    let app = run_turn_into_app(agent, config).await;

    assert!(
        (app.usage.total_cost - 1.00).abs() < 1e-9,
        "operator rate ($1.00/M) should win over the catalog's $3.00/M, got ${:.4}",
        app.usage.total_cost
    );
}

/// A `[pricing]` table that does not mention the model in play must leave
/// catalog pricing alone rather than zeroing it.
#[tokio::test]
async fn pricing_for_another_model_leaves_catalog_pricing_intact() {
    let config = config_with_pricing(
        r#"
            [pricing."some-other-model"]
            input_per_million = 1.00
        "#,
    );
    let agent = agent_for("claude-sonnet-4-6", |options| config.apply_pricing(options));

    let app = run_turn_into_app(agent, config).await;

    assert!(
        (app.usage.total_cost - 3.00).abs() < 1e-9,
        "expected catalog pricing ($3.00/M), got ${:.4}",
        app.usage.total_cost
    );
}

/// Without a `[pricing]` section the catalog still prices known models — the
/// #1103 behaviour must survive this change.
#[tokio::test]
async fn catalog_pricing_still_applies_with_no_pricing_section() {
    let config = TuiConfig::default();
    assert!(config.pricing.is_empty());
    let agent = agent_for("claude-sonnet-4-6", |options| config.apply_pricing(options));

    let app = run_turn_into_app(agent, config).await;

    assert!(
        (app.usage.total_cost - 3.00).abs() < 1e-9,
        "{}",
        app.usage.total_cost
    );
}

/// An unknown model with no declared rates honestly reports zero rather than
/// inventing a number.
#[tokio::test]
async fn unknown_model_without_declared_rates_reports_zero_cost() {
    let config = TuiConfig::default();
    let agent = agent_for("totally-unknown-model", |options| {
        config.apply_pricing(options)
    });

    let app = run_turn_into_app(agent, config).await;

    assert_eq!(app.usage.total_input_tokens, 1_000_000);
    assert!((app.usage.total_cost).abs() < 1e-9);
}
