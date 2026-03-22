//! Integration tests for `LoopPolicy`.

mod common;

use std::sync::Arc;

use swink_agent::{
    AgentOptions, AssistantMessageEvent, ComposedPolicy, Cost, CostCapPolicy, MaxTurnsPolicy,
    ModelSpec, StopReason, SubAgent, Usage, stream::StreamFn,
};

use common::{MockStreamFn, MockTool, default_model, text_only_events, tool_call_events};

#[tokio::test]
async fn max_turns_limits_agent_loop() {
    // MockStreamFn always returns tool calls, then text after enough turns
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
        .with_loop_policy(MaxTurnsPolicy::new(2));

    let mut agent = swink_agent::Agent::new(options);
    let result = agent.prompt_text("go").await;

    // The agent should complete (policy stops it) — either Ok or the loop terminated
    // The key assertion is that we don't run all 6 responses
    assert!(result.is_ok());
    // Tool was executed at most 2 times (2 turns)
    assert!(tool.execution_count() <= 2);
}

#[tokio::test]
async fn cost_cap_stops_agent() {
    // Each response costs 0.005
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
                cost: Cost {
                    total: 0.005,
                    ..Cost::default()
                },
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
                cost: Cost {
                    total: 0.005,
                    ..Cost::default()
                },
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

    // Cost cap at 0.01 — should allow ~2 turns (0.005 each)
    let options = AgentOptions::new_simple("test", default_model(), stream_fn)
        .with_tools(vec![tool.clone()])
        .with_loop_policy(CostCapPolicy::new(0.01));

    let mut agent = swink_agent::Agent::new(options);
    let result = agent.prompt_text("go").await;

    assert!(result.is_ok());
    // Should have stopped before all 3 tool executions
    assert!(tool.execution_count() <= 2);
}

#[tokio::test]
async fn composed_policy_applies_all() {
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

    // Max turns = 5 (lenient), cost cap = 0.0 (strict) — cost cap wins
    let policy = ComposedPolicy::new(vec![
        Box::new(MaxTurnsPolicy::new(5)),
        Box::new(CostCapPolicy::new(0.0)),
    ]);

    let options = AgentOptions::new_simple("test", default_model(), stream_fn)
        .with_tools(vec![tool.clone()])
        .with_loop_policy(policy);

    let mut agent = swink_agent::Agent::new(options);
    let result = agent.prompt_text("go").await;

    assert!(result.is_ok());
    // Cost cap of 0.0 should stop very quickly
    assert!(tool.execution_count() <= 1);
}

#[tokio::test]
async fn policy_with_sub_agent() {
    // Parent responses: one tool call to the sub-agent, then text
    let parent_responses = vec![
        tool_call_events("sub-call", "researcher", r#"{"prompt":"research this"}"#),
        text_only_events("parent final answer"),
    ];
    let parent_stream: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(parent_responses));

    // Sub-agent stream
    let sub_stream: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![text_only_events(
        "sub-agent result",
    )]));

    let sfn = sub_stream.clone();
    let sub = Arc::new(
        SubAgent::new("researcher", "Researcher", "Research sub-agent")
            .with_options(move || AgentOptions::new_simple("sub", default_model(), Arc::clone(&sfn))),
    );

    // Parent with max 3 turns
    let options = AgentOptions::new_simple("parent", default_model(), parent_stream)
        .with_tools(vec![sub as Arc<dyn swink_agent::AgentTool>])
        .with_loop_policy(MaxTurnsPolicy::new(3));

    let mut agent = swink_agent::Agent::new(options);
    let result = agent.prompt_text("research and answer").await;

    assert!(result.is_ok());
}
