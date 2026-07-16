//! Integration tests for policy slots — moved from swink-agent core.

mod common;

use std::sync::Arc;

use swink_agent::{
    AgentOptions, AssistantMessageEvent, Cost, ModelSpec, StopReason, StreamFn, SubAgent, Usage,
};
use swink_agent_policies::{BudgetPolicy, MaxTurnsPolicy};

use common::{MockStreamFn, MockTool, default_model, text_only_events, tool_call_events};

#[tokio::test]
async fn max_turns_limits_agent_loop() {
    let responses = vec![
        tool_call_events("call-1", "mock_tool", "{}"),
        text_only_events("turn 1 tool result processed"),
        tool_call_events("call-2", "mock_tool", "{}"),
        text_only_events("turn 2 tool result processed"),
        tool_call_events("call-3", "mock_tool", "{}"),
        text_only_events("turn 3 should not happen"),
    ];
    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(responses));
    let tool = Arc::new(MockTool::new("mock_tool"));

    let options = AgentOptions::new_simple("test", default_model(), stream_fn)
        .with_tools(vec![tool.clone()])
        .with_pre_turn_policy(MaxTurnsPolicy::new(2));

    let mut agent = swink_agent::Agent::new(options);
    let result = agent.prompt_text("go").await;

    assert!(result.is_ok());
    assert!(tool.execution_count() <= 2);
}

#[tokio::test]
async fn cost_cap_stops_agent() {
    let make_events = |text: &str| -> Vec<AssistantMessageEvent> {
        vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::TextStart { content_index: 0 },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: text.to_string(),
            },
            AssistantMessageEvent::TextEnd { content_index: 0 },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
                cost: Cost::default().with_total(0.005),
            },
        ]
    };

    let tool_events = |id: &str| -> Vec<AssistantMessageEvent> {
        vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::ToolCallStart {
                content_index: 0,
                id: id.to_string(),
                name: "mock_tool".to_string(),
            },
            AssistantMessageEvent::ToolCallDelta {
                content_index: 0,
                delta: "{}".to_string(),
            },
            AssistantMessageEvent::ToolCallEnd { content_index: 0 },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
                cost: Cost::default().with_total(0.005),
            },
        ]
    };

    let responses = vec![
        tool_events("c1"),
        make_events("after tool 1"),
        tool_events("c2"),
        make_events("after tool 2"),
        tool_events("c3"),
        make_events("after tool 3"),
    ];
    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(responses));
    let tool = Arc::new(MockTool::new("mock_tool"));

    let options = AgentOptions::new_simple("test", default_model(), stream_fn)
        .with_tools(vec![tool.clone()])
        .with_pre_turn_policy(BudgetPolicy::new().max_cost(0.01));

    let mut agent = swink_agent::Agent::new(options);
    let result = agent.prompt_text("go").await;

    assert!(result.is_ok());
    assert!(tool.execution_count() <= 2);
}

/// Regression test for issue #1100.
///
/// Every built-in remote adapter reports token `Usage` but emits
/// `Cost::default()` on assistant messages — only the proxy adapter passes
/// real billed cost through. The loop used to accumulate that zero verbatim,
/// so `BudgetPolicy::max_cost` never fired against any real provider. The loop
/// now prices unpriced messages from the model catalog, so the ceiling engages.
///
/// The mock below mimics a real adapter exactly: real `Usage`, zero `Cost`.
/// At $3.00/M input tokens for `claude-sonnet-4-6`, each turn costs $3.00, so
/// a $5.00 ceiling must stop the loop before the third turn. Six responses are
/// scripted, so stopping at two proves the budget — not script exhaustion —
/// ended the run. Before the fix this ran all six.
#[tokio::test]
async fn cost_cap_stops_agent_when_adapter_reports_no_cost() {
    let adapter_events_without_cost = |id: &str| -> Vec<AssistantMessageEvent> {
        vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::ToolCallStart {
                content_index: 0,
                id: id.to_string(),
                name: "mock_tool".to_string(),
            },
            AssistantMessageEvent::ToolCallDelta {
                content_index: 0,
                delta: "{}".to_string(),
            },
            AssistantMessageEvent::ToolCallEnd { content_index: 0 },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::ToolUse,
                usage: Usage::default().with_input(1_000_000),
                // What every built-in remote adapter actually emits.
                cost: Cost::default(),
            },
        ]
    };

    let responses = vec![
        adapter_events_without_cost("c1"),
        adapter_events_without_cost("c2"),
        adapter_events_without_cost("c3"),
        adapter_events_without_cost("c4"),
        adapter_events_without_cost("c5"),
        adapter_events_without_cost("c6"),
    ];
    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(responses));
    let tool = Arc::new(MockTool::new("mock_tool"));

    // A model with catalog pricing: $3.00 per million input tokens.
    let model = ModelSpec::new("anthropic", "claude-sonnet-4-6");
    let options = AgentOptions::new_simple("test", model, stream_fn)
        .with_tools(vec![tool.clone()])
        .with_pre_turn_policy(BudgetPolicy::new().max_cost(5.0));

    let mut agent = swink_agent::Agent::new(options);
    let result = agent.prompt_text("go").await;

    assert!(result.is_ok());
    assert_eq!(
        tool.execution_count(),
        2,
        "budget should stop the loop once accumulated cost ($6.00 after two \
         turns) crosses the $5.00 ceiling; running longer means adapter-\
         reported zero cost is still being accumulated verbatim (issue #1100)"
    );
}

/// Companion to `cost_cap_stops_agent_when_adapter_reports_no_cost`: an adapter
/// that prices its own response (as the proxy adapter does) keeps precedence
/// over catalog pricing.
#[tokio::test]
async fn adapter_supplied_cost_takes_precedence_over_catalog_pricing() {
    let events_with_adapter_cost = |id: &str| -> Vec<AssistantMessageEvent> {
        vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::ToolCallStart {
                content_index: 0,
                id: id.to_string(),
                name: "mock_tool".to_string(),
            },
            AssistantMessageEvent::ToolCallDelta {
                content_index: 0,
                delta: "{}".to_string(),
            },
            AssistantMessageEvent::ToolCallEnd { content_index: 0 },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::ToolUse,
                // Catalog pricing for this usage would be $3.00/turn; the
                // adapter says the call was billed at $1.00. The loop must
                // trust the adapter, so the $5.00 ceiling takes five turns
                // rather than two to trip.
                usage: Usage::default().with_input(1_000_000),
                cost: Cost::default().with_input(1.0).with_total(1.0),
            },
        ]
    };

    let responses = (1..=8)
        .map(|i| events_with_adapter_cost(&format!("c{i}")))
        .collect();
    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(responses));
    let tool = Arc::new(MockTool::new("mock_tool"));

    let model = ModelSpec::new("anthropic", "claude-sonnet-4-6");
    let options = AgentOptions::new_simple("test", model, stream_fn)
        .with_tools(vec![tool.clone()])
        .with_pre_turn_policy(BudgetPolicy::new().max_cost(5.0));

    let mut agent = swink_agent::Agent::new(options);
    let result = agent.prompt_text("go").await;

    assert!(result.is_ok());
    assert_eq!(
        tool.execution_count(),
        5,
        "adapter-supplied cost ($1.00/turn) must win over catalog pricing \
         ($3.00/turn), so the $5.00 ceiling trips on the sixth turn"
    );
}

#[tokio::test]
async fn composed_policies_apply_all() {
    let responses = vec![
        tool_call_events("c1", "mock_tool", "{}"),
        text_only_events("done 1"),
        tool_call_events("c2", "mock_tool", "{}"),
        text_only_events("done 2"),
        tool_call_events("c3", "mock_tool", "{}"),
        text_only_events("done 3"),
    ];
    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(responses));
    let tool = Arc::new(MockTool::new("mock_tool"));

    let options = AgentOptions::new_simple("test", default_model(), stream_fn)
        .with_tools(vec![tool.clone()])
        .with_pre_turn_policy(MaxTurnsPolicy::new(5))
        .with_pre_turn_policy(BudgetPolicy::new().max_cost(0.0));

    let mut agent = swink_agent::Agent::new(options);
    let result = agent.prompt_text("go").await;

    assert!(result.is_ok());
    assert!(tool.execution_count() <= 1);
}

#[tokio::test]
async fn policy_with_sub_agent() {
    let parent_responses = vec![
        tool_call_events("sub-call", "researcher", r#"{"prompt":"research this"}"#),
        text_only_events("parent final answer"),
    ];
    let parent_stream: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(parent_responses));

    let sub_stream: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![text_only_events(
        "sub-agent result",
    )]));

    let sfn = sub_stream.clone();
    let sub = Arc::new(
        SubAgent::new("researcher", "Researcher", "Research sub-agent").with_options(move || {
            AgentOptions::new_simple("sub", default_model(), Arc::clone(&sfn))
        }),
    );

    let options = AgentOptions::new_simple("parent", default_model(), parent_stream)
        .with_tools(vec![sub as Arc<dyn swink_agent::AgentTool>])
        .with_pre_turn_policy(MaxTurnsPolicy::new(3));

    let mut agent = swink_agent::Agent::new(options);
    let result = agent.prompt_text("research and answer").await;

    assert!(result.is_ok());
}
