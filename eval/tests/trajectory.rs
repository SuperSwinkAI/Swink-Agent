//! Acceptance tests for US1: Trajectory Collection (spec 023).

mod common;

use std::sync::Arc;

use futures::stream;
use serde_json::json;
use swink_agent::{
    AgentEvent, AssistantMessage, ContentBlock, Cost, ModelSpec, StopReason, TurnSnapshot, Usage,
};
use swink_agent_eval::TrajectoryCollector;

/// Build a minimal `AssistantMessage` with optional text content and tool call blocks.
fn assistant_msg(
    text: Option<&str>,
    tool_calls: &[(&str, &str, serde_json::Value)],
    stop: StopReason,
) -> AssistantMessage {
    let mut content: Vec<ContentBlock> = tool_calls
        .iter()
        .map(|(id, name, args)| ContentBlock::ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: args.clone(),
            partial_json: None,
        })
        .collect();
    if let Some(t) = text {
        content.insert(
            0,
            ContentBlock::Text {
                text: t.to_string(),
            },
        );
    }
    AssistantMessage {
        content,
        provider: "test".to_string(),
        model_id: "test-model".to_string(),
        usage: Usage {
            total: 100,
            ..Default::default()
        },
        cost: Cost {
            total: 0.01,
            ..Default::default()
        },
        stop_reason: stop,
        error_message: None,
        error_kind: None,
        timestamp: 0,
        cache_hint: None,
    }
}

fn empty_snapshot() -> TurnSnapshot {
    TurnSnapshot {
        turn_index: 0,
        messages: Arc::new(vec![]),
        usage: Usage::default(),
        cost: Cost::default(),
        stop_reason: StopReason::Stop,
        state_delta: None,
    }
}

fn tool_result(id: &str, content: &str) -> swink_agent::ToolResultMessage {
    swink_agent::ToolResultMessage {
        tool_call_id: id.to_string(),
        content: vec![ContentBlock::Text {
            text: content.to_string(),
        }],
        is_error: false,
        timestamp: 0,
        details: serde_json::Value::Null,
        cache_hint: None,
    }
}

fn tool_result_error(id: &str, content: &str) -> swink_agent::ToolResultMessage {
    swink_agent::ToolResultMessage {
        tool_call_id: id.to_string(),
        content: vec![ContentBlock::Text {
            text: content.to_string(),
        }],
        is_error: true,
        timestamp: 0,
        details: serde_json::Value::Null,
        cache_hint: None,
    }
}

/// AS-1.1: Multi-tool invocations captured with name, inputs, and result.
#[tokio::test]
async fn us1_multi_tool_invocations_captured() {
    let events = vec![
        AgentEvent::AgentStart,
        AgentEvent::BeforeLlmCall {
            system_prompt: "test".to_string(),
            messages: vec![],
            model: ModelSpec::new("test", "test-model"),
        },
        AgentEvent::TurnStart,
        AgentEvent::ToolExecutionStart {
            id: "call_1".to_string(),
            name: "read_file".to_string(),
            arguments: json!({"path": "a.txt"}),
        },
        AgentEvent::ToolExecutionStart {
            id: "call_2".to_string(),
            name: "write_file".to_string(),
            arguments: json!({"path": "b.txt", "content": "hello"}),
        },
        AgentEvent::TurnEnd {
            assistant_message: assistant_msg(
                None,
                &[
                    ("call_1", "read_file", json!({"path": "a.txt"})),
                    (
                        "call_2",
                        "write_file",
                        json!({"path": "b.txt", "content": "hello"}),
                    ),
                ],
                StopReason::ToolUse,
            ),
            tool_results: vec![
                tool_result("call_1", "file contents"),
                tool_result("call_2", "ok"),
            ],
            reason: swink_agent::TurnEndReason::ToolsExecuted,
            snapshot: empty_snapshot(),
        },
        AgentEvent::AgentEnd {
            messages: Arc::new(vec![]),
        },
    ];

    let invocation = TrajectoryCollector::collect_from_stream(stream::iter(events)).await;

    assert_eq!(invocation.turns.len(), 1);
    let turn = &invocation.turns[0];
    assert_eq!(turn.tool_calls.len(), 2);
    assert_eq!(turn.tool_calls[0].name, "read_file");
    assert_eq!(turn.tool_calls[0].arguments, json!({"path": "a.txt"}));
    assert_eq!(turn.tool_calls[1].name, "write_file");
    assert_eq!(
        turn.tool_calls[1].arguments,
        json!({"path": "b.txt", "content": "hello"})
    );
}

/// AS-1.2: Multi-turn chronological ordering.
#[tokio::test]
async fn us1_multi_turn_chronological_ordering() {
    let events = vec![
        AgentEvent::AgentStart,
        AgentEvent::BeforeLlmCall {
            system_prompt: "test".to_string(),
            messages: vec![],
            model: ModelSpec::new("test", "test-model"),
        },
        // Turn 0
        AgentEvent::TurnStart,
        AgentEvent::ToolExecutionStart {
            id: "c1".to_string(),
            name: "search".to_string(),
            arguments: json!({}),
        },
        AgentEvent::TurnEnd {
            assistant_message: assistant_msg(
                None,
                &[("c1", "search", json!({}))],
                StopReason::ToolUse,
            ),
            tool_results: vec![tool_result("c1", "found")],
            reason: swink_agent::TurnEndReason::ToolsExecuted,
            snapshot: empty_snapshot(),
        },
        // Turn 1
        AgentEvent::TurnStart,
        AgentEvent::ToolExecutionStart {
            id: "c2".to_string(),
            name: "read_file".to_string(),
            arguments: json!({}),
        },
        AgentEvent::TurnEnd {
            assistant_message: assistant_msg(
                None,
                &[("c2", "read_file", json!({}))],
                StopReason::ToolUse,
            ),
            tool_results: vec![tool_result("c2", "data")],
            reason: swink_agent::TurnEndReason::ToolsExecuted,
            snapshot: empty_snapshot(),
        },
        AgentEvent::AgentEnd {
            messages: Arc::new(vec![]),
        },
    ];

    let invocation = TrajectoryCollector::collect_from_stream(stream::iter(events)).await;

    assert_eq!(invocation.turns.len(), 2);
    assert_eq!(invocation.turns[0].turn_index, 0);
    assert_eq!(invocation.turns[0].tool_calls[0].name, "search");
    assert_eq!(invocation.turns[1].turn_index, 1);
    assert_eq!(invocation.turns[1].tool_calls[0].name, "read_file");
}

/// AS-1.3: Failed tool call captured, not silently dropped.
#[tokio::test]
async fn us1_failed_tool_call_captured() {
    let events = vec![
        AgentEvent::AgentStart,
        AgentEvent::BeforeLlmCall {
            system_prompt: "test".to_string(),
            messages: vec![],
            model: ModelSpec::new("test", "test-model"),
        },
        AgentEvent::TurnStart,
        AgentEvent::ToolExecutionStart {
            id: "c1".to_string(),
            name: "delete_file".to_string(),
            arguments: json!({"path": "/nonexistent"}),
        },
        AgentEvent::TurnEnd {
            assistant_message: assistant_msg(
                None,
                &[("c1", "delete_file", json!({"path": "/nonexistent"}))],
                StopReason::ToolUse,
            ),
            tool_results: vec![tool_result_error("c1", "file not found")],
            reason: swink_agent::TurnEndReason::ToolsExecuted,
            snapshot: empty_snapshot(),
        },
        AgentEvent::AgentEnd {
            messages: Arc::new(vec![]),
        },
    ];

    let invocation = TrajectoryCollector::collect_from_stream(stream::iter(events)).await;

    assert_eq!(invocation.turns.len(), 1);
    let turn = &invocation.turns[0];
    assert_eq!(turn.tool_calls.len(), 1);
    assert_eq!(turn.tool_calls[0].name, "delete_file");
    assert_eq!(turn.tool_results.len(), 1);
    assert!(turn.tool_results[0].is_error);
}

/// AS-1.4: Text-only response (no tool calls) — zero invocations, response captured.
#[tokio::test]
async fn us1_text_only_response_captured() {
    let events = vec![
        AgentEvent::AgentStart,
        AgentEvent::BeforeLlmCall {
            system_prompt: "test".to_string(),
            messages: vec![],
            model: ModelSpec::new("test", "test-model"),
        },
        AgentEvent::TurnStart,
        AgentEvent::TurnEnd {
            assistant_message: assistant_msg(Some("The answer is 42."), &[], StopReason::Stop),
            tool_results: vec![],
            reason: swink_agent::TurnEndReason::Complete,
            snapshot: empty_snapshot(),
        },
        AgentEvent::AgentEnd {
            messages: Arc::new(vec![]),
        },
    ];

    let invocation = TrajectoryCollector::collect_from_stream(stream::iter(events)).await;

    assert_eq!(invocation.turns.len(), 1);
    assert!(invocation.turns[0].tool_calls.is_empty());
    assert_eq!(
        invocation.final_response.as_deref(),
        Some("The answer is 42.")
    );
}

#[tokio::test]
async fn rejected_tool_call_without_execution_start_is_still_captured() {
    let events = vec![
        AgentEvent::AgentStart,
        AgentEvent::BeforeLlmCall {
            system_prompt: "test".to_string(),
            messages: vec![],
            model: ModelSpec::new("test", "test-model"),
        },
        AgentEvent::TurnStart,
        AgentEvent::ToolApprovalRequested {
            id: "call_reject".to_string(),
            name: "write_file".to_string(),
            arguments: json!({"path": "draft.txt"}),
        },
        AgentEvent::ToolApprovalResolved {
            id: "call_reject".to_string(),
            name: "write_file".to_string(),
            approved: false,
        },
        AgentEvent::ToolExecutionEnd {
            id: "call_reject".to_string(),
            name: "write_file".to_string(),
            result: swink_agent::AgentToolResult::error(
                "Tool call 'write_file' was rejected by the approval gate.",
            ),
            is_error: true,
        },
        AgentEvent::TurnEnd {
            assistant_message: assistant_msg(
                None,
                &[("call_reject", "write_file", json!({"path": "draft.txt"}))],
                StopReason::ToolUse,
            ),
            tool_results: vec![tool_result_error(
                "call_reject",
                "Tool call 'write_file' was rejected by the approval gate.",
            )],
            reason: swink_agent::TurnEndReason::ToolsExecuted,
            snapshot: empty_snapshot(),
        },
        AgentEvent::AgentEnd {
            messages: Arc::new(vec![]),
        },
    ];

    let invocation = TrajectoryCollector::collect_from_stream(stream::iter(events)).await;

    assert_eq!(invocation.turns.len(), 1);
    let turn = &invocation.turns[0];
    assert_eq!(turn.tool_calls.len(), 1);
    assert_eq!(turn.tool_calls[0].id, "call_reject");
    assert_eq!(turn.tool_calls[0].name, "write_file");
    assert_eq!(turn.tool_calls[0].arguments, json!({"path": "draft.txt"}));
    assert!(turn.tool_results[0].is_error);
}

#[tokio::test]
async fn pre_dispatch_skip_without_execution_start_is_still_captured() {
    let events = vec![
        AgentEvent::AgentStart,
        AgentEvent::BeforeLlmCall {
            system_prompt: "test".to_string(),
            messages: vec![],
            model: ModelSpec::new("test", "test-model"),
        },
        AgentEvent::TurnStart,
        AgentEvent::ToolExecutionEnd {
            id: "call_skip".to_string(),
            name: "delete_file".to_string(),
            result: swink_agent::AgentToolResult::error("blocked by pre-dispatch policy"),
            is_error: true,
        },
        AgentEvent::TurnEnd {
            assistant_message: assistant_msg(
                None,
                &[("call_skip", "delete_file", json!({"path": "secret.txt"}))],
                StopReason::ToolUse,
            ),
            tool_results: vec![tool_result_error(
                "call_skip",
                "blocked by pre-dispatch policy",
            )],
            reason: swink_agent::TurnEndReason::ToolsExecuted,
            snapshot: empty_snapshot(),
        },
        AgentEvent::AgentEnd {
            messages: Arc::new(vec![]),
        },
    ];

    let invocation = TrajectoryCollector::collect_from_stream(stream::iter(events)).await;

    assert_eq!(invocation.turns.len(), 1);
    let turn = &invocation.turns[0];
    assert_eq!(turn.tool_calls.len(), 1);
    assert_eq!(turn.tool_calls[0].id, "call_skip");
    assert_eq!(turn.tool_calls[0].name, "delete_file");
    assert_eq!(turn.tool_calls[0].arguments, json!({"path": "secret.txt"}));
    assert!(turn.tool_results[0].is_error);
}
