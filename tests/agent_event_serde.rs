//! Tests that every `AgentEvent` variant serializes to JSON without error.

mod common;

use std::borrow::Cow;
use std::sync::Arc;

use serde_json::json;
use swink_agent::{
    AgentEvent, AgentMessage, AgentToolResult, AssistantMessage, AssistantMessageDelta,
    CompactionReport, ContentBlock, Cost, Emission, LlmMessage, ModelSpec, StopReason,
    ToolResultMessage, TurnEndReason, TurnSnapshot, Usage, UserMessage,
};

use common::default_model;

/// Helper: build a minimal valid `AssistantMessage`.
fn minimal_assistant_message() -> AssistantMessage {
    AssistantMessage {
        content: vec![ContentBlock::Text {
            text: "hello".into(),
        }],
        provider: "test".into(),
        model_id: "test-model".into(),
        usage: Usage::default(),
        cost: Cost::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
        cache_hint: None,
    }
}

/// Helper: build a minimal `TurnSnapshot`.
fn minimal_snapshot() -> TurnSnapshot {
    TurnSnapshot {
        turn_index: 0,
        messages: Arc::new(vec![LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "hi".into(),
            }],
            timestamp: 0,
            cache_hint: None,
        })]),
        usage: Usage::default(),
        cost: Cost::default(),
        stop_reason: StopReason::Stop,
        state_delta: None,
    }
}

#[test]
fn all_agent_event_variants_serialize_to_json() {
    let model = default_model();

    let events: Vec<(&str, AgentEvent)> = vec![
        ("AgentStart", AgentEvent::AgentStart),
        (
            "AgentEnd",
            AgentEvent::AgentEnd {
                messages: Arc::new(vec![]),
            },
        ),
        ("TurnStart", AgentEvent::TurnStart),
        (
            "TurnEnd",
            AgentEvent::TurnEnd {
                assistant_message: minimal_assistant_message(),
                tool_results: vec![ToolResultMessage {
                    tool_call_id: "tc1".into(),
                    content: vec![ContentBlock::Text {
                        text: "ok".into(),
                    }],
                    is_error: false,
                    timestamp: 0,
                    details: json!(null),
                    cache_hint: None,
                }],
                reason: TurnEndReason::Complete,
                snapshot: minimal_snapshot(),
            },
        ),
        (
            "BeforeLlmCall",
            AgentEvent::BeforeLlmCall {
                system_prompt: "You are helpful.".into(),
                messages: vec![LlmMessage::User(UserMessage {
                    content: vec![ContentBlock::Text {
                        text: "hi".into(),
                    }],
                    timestamp: 0,
                    cache_hint: None,
                })],
                model: model.clone(),
            },
        ),
        ("MessageStart", AgentEvent::MessageStart),
        (
            "MessageUpdate_Text",
            AgentEvent::MessageUpdate {
                delta: AssistantMessageDelta::Text {
                    content_index: 0,
                    delta: Cow::Borrowed("chunk"),
                },
            },
        ),
        (
            "MessageUpdate_Thinking",
            AgentEvent::MessageUpdate {
                delta: AssistantMessageDelta::Thinking {
                    content_index: 0,
                    delta: Cow::Borrowed("reasoning"),
                },
            },
        ),
        (
            "MessageUpdate_ToolCall",
            AgentEvent::MessageUpdate {
                delta: AssistantMessageDelta::ToolCall {
                    content_index: 0,
                    delta: Cow::Borrowed("{\"key\":\"value\"}"),
                },
            },
        ),
        (
            "MessageEnd",
            AgentEvent::MessageEnd {
                message: minimal_assistant_message(),
            },
        ),
        (
            "ToolExecutionStart",
            AgentEvent::ToolExecutionStart {
                id: "tc1".into(),
                name: "my_tool".into(),
                arguments: json!({"path": "/tmp"}),
            },
        ),
        (
            "ToolExecutionUpdate",
            AgentEvent::ToolExecutionUpdate {
                partial: AgentToolResult::text("partial output"),
            },
        ),
        (
            "ToolExecutionEnd",
            AgentEvent::ToolExecutionEnd {
                result: AgentToolResult::text("done"),
                is_error: false,
            },
        ),
        (
            "ToolApprovalRequested",
            AgentEvent::ToolApprovalRequested {
                id: "tc1".into(),
                name: "dangerous_tool".into(),
                arguments: json!({}),
            },
        ),
        (
            "ToolApprovalResolved",
            AgentEvent::ToolApprovalResolved {
                id: "tc1".into(),
                name: "dangerous_tool".into(),
                approved: true,
            },
        ),
        (
            "ContextCompacted",
            AgentEvent::ContextCompacted {
                report: CompactionReport {
                    dropped_count: 5,
                    tokens_before: 10000,
                    tokens_after: 5000,
                    overflow: false,
                },
            },
        ),
        (
            "ModelFallback",
            AgentEvent::ModelFallback {
                from_model: model.clone(),
                to_model: ModelSpec::new("openai", "gpt-4"),
            },
        ),
        (
            "ModelCycled",
            AgentEvent::ModelCycled {
                old: model.clone(),
                new: ModelSpec::new("openai", "gpt-4"),
                reason: "throttled".into(),
            },
        ),
        (
            "StateChanged",
            AgentEvent::StateChanged {
                delta: swink_agent::StateDelta::default(),
            },
        ),
        (
            "Custom",
            AgentEvent::Custom(Emission::new("test_event", json!({"key": "value"}))),
        ),
    ];

    for (label, event) in &events {
        let result = serde_json::to_value(event);
        assert!(
            result.is_ok(),
            "Failed to serialize AgentEvent variant '{label}': {:?}",
            result.err()
        );
    }
}

#[test]
fn agent_start_serializes_with_event_tag() {
    let val = serde_json::to_value(&AgentEvent::AgentStart).unwrap();
    assert_eq!(val["event"], "agent_start");
}

#[test]
fn turn_end_serializes_with_expected_keys() {
    let event = AgentEvent::TurnEnd {
        assistant_message: minimal_assistant_message(),
        tool_results: vec![],
        reason: TurnEndReason::Complete,
        snapshot: minimal_snapshot(),
    };
    let val = serde_json::to_value(&event).unwrap();
    assert_eq!(val["event"], "turn_end");
    assert!(val.get("assistant_message").is_some());
    assert!(val.get("tool_results").is_some());
    assert!(val.get("reason").is_some());
    assert!(val.get("snapshot").is_some());
}

#[test]
fn tool_execution_start_serializes_with_expected_keys() {
    let event = AgentEvent::ToolExecutionStart {
        id: "tc99".into(),
        name: "bash".into(),
        arguments: json!({"command": "ls"}),
    };
    let val = serde_json::to_value(&event).unwrap();
    assert_eq!(val["event"], "tool_execution_start");
    assert_eq!(val["id"], "tc99");
    assert_eq!(val["name"], "bash");
    assert_eq!(val["arguments"]["command"], "ls");
}

#[test]
fn context_compacted_serializes_report_fields() {
    let event = AgentEvent::ContextCompacted {
        report: CompactionReport {
            dropped_count: 10,
            tokens_before: 20000,
            tokens_after: 8000,
            overflow: true,
        },
    };
    let val = serde_json::to_value(&event).unwrap();
    assert_eq!(val["event"], "context_compacted");
    let report = &val["report"];
    assert_eq!(report["dropped_count"], 10);
    assert_eq!(report["tokens_before"], 20000);
    assert_eq!(report["tokens_after"], 8000);
    assert_eq!(report["overflow"], true);
}

#[test]
fn custom_event_serializes_emission() {
    let event = AgentEvent::Custom(Emission::new("progress", json!({"percent": 50})));
    let val = serde_json::to_value(&event).unwrap();
    assert_eq!(val["event"], "custom");
    // The Emission fields should be present (flattened or nested depending on serde config).
    // At minimum, the value should be valid JSON.
    assert!(val.to_string().contains("progress") || val.to_string().contains("percent"));
}

#[test]
fn message_update_delta_variants_have_type_tag() {
    let text_event = AgentEvent::MessageUpdate {
        delta: AssistantMessageDelta::Text {
            content_index: 0,
            delta: Cow::Borrowed("hello"),
        },
    };
    let val = serde_json::to_value(&text_event).unwrap();
    assert_eq!(val["event"], "message_update");
    assert_eq!(val["delta"]["type"], "text");

    let thinking_event = AgentEvent::MessageUpdate {
        delta: AssistantMessageDelta::Thinking {
            content_index: 1,
            delta: Cow::Borrowed("hmm"),
        },
    };
    let val = serde_json::to_value(&thinking_event).unwrap();
    assert_eq!(val["delta"]["type"], "thinking");

    let tool_event = AgentEvent::MessageUpdate {
        delta: AssistantMessageDelta::ToolCall {
            content_index: 2,
            delta: Cow::Borrowed("{}"),
        },
    };
    let val = serde_json::to_value(&tool_event).unwrap();
    assert_eq!(val["delta"]["type"], "tool_call");
}

#[test]
fn model_fallback_serializes_both_models() {
    let event = AgentEvent::ModelFallback {
        from_model: ModelSpec::new("anthropic", "claude-3"),
        to_model: ModelSpec::new("openai", "gpt-4o"),
    };
    let val = serde_json::to_value(&event).unwrap();
    assert_eq!(val["event"], "model_fallback");
    assert!(val.get("from_model").is_some());
    assert!(val.get("to_model").is_some());
}

#[test]
fn agent_end_serializes_messages_array() {
    let msgs: Vec<AgentMessage> = vec![common::user_msg("hello")];
    let event = AgentEvent::AgentEnd {
        messages: Arc::new(msgs),
    };
    let val = serde_json::to_value(&event).unwrap();
    assert_eq!(val["event"], "agent_end");
    assert!(val["messages"].is_array());
}

// ─── Deserialization tests ──────────────────────────────────────────────────

#[test]
fn agent_event_roundtrip_all_variants() {
    let model = default_model();

    let events: Vec<(&str, AgentEvent)> = vec![
        ("AgentStart", AgentEvent::AgentStart),
        (
            "AgentEnd",
            AgentEvent::AgentEnd {
                messages: Arc::new(vec![]),
            },
        ),
        ("TurnStart", AgentEvent::TurnStart),
        (
            "TurnEnd",
            AgentEvent::TurnEnd {
                assistant_message: minimal_assistant_message(),
                tool_results: vec![ToolResultMessage {
                    tool_call_id: "tc1".into(),
                    content: vec![ContentBlock::Text {
                        text: "ok".into(),
                    }],
                    is_error: false,
                    timestamp: 0,
                    details: json!(null),
                    cache_hint: None,
                }],
                reason: TurnEndReason::Complete,
                snapshot: minimal_snapshot(),
            },
        ),
        (
            "BeforeLlmCall",
            AgentEvent::BeforeLlmCall {
                system_prompt: "You are helpful.".into(),
                messages: vec![LlmMessage::User(UserMessage {
                    content: vec![ContentBlock::Text {
                        text: "hi".into(),
                    }],
                    timestamp: 0,
                    cache_hint: None,
                })],
                model: model.clone(),
            },
        ),
        ("MessageStart", AgentEvent::MessageStart),
        (
            "MessageUpdate_Text",
            AgentEvent::MessageUpdate {
                delta: AssistantMessageDelta::Text {
                    content_index: 0,
                    delta: Cow::Borrowed("chunk"),
                },
            },
        ),
        (
            "MessageUpdate_Thinking",
            AgentEvent::MessageUpdate {
                delta: AssistantMessageDelta::Thinking {
                    content_index: 0,
                    delta: Cow::Borrowed("reasoning"),
                },
            },
        ),
        (
            "MessageUpdate_ToolCall",
            AgentEvent::MessageUpdate {
                delta: AssistantMessageDelta::ToolCall {
                    content_index: 0,
                    delta: Cow::Borrowed("{\"key\":\"value\"}"),
                },
            },
        ),
        (
            "MessageEnd",
            AgentEvent::MessageEnd {
                message: minimal_assistant_message(),
            },
        ),
        (
            "ToolExecutionStart",
            AgentEvent::ToolExecutionStart {
                id: "tc1".into(),
                name: "my_tool".into(),
                arguments: json!({"path": "/tmp"}),
            },
        ),
        (
            "ToolExecutionUpdate",
            AgentEvent::ToolExecutionUpdate {
                partial: AgentToolResult::text("partial output"),
            },
        ),
        (
            "ToolExecutionEnd",
            AgentEvent::ToolExecutionEnd {
                result: AgentToolResult::text("done"),
                is_error: false,
            },
        ),
        (
            "ToolApprovalRequested",
            AgentEvent::ToolApprovalRequested {
                id: "tc1".into(),
                name: "dangerous_tool".into(),
                arguments: json!({}),
            },
        ),
        (
            "ToolApprovalResolved",
            AgentEvent::ToolApprovalResolved {
                id: "tc1".into(),
                name: "dangerous_tool".into(),
                approved: true,
            },
        ),
        (
            "ContextCompacted",
            AgentEvent::ContextCompacted {
                report: CompactionReport {
                    dropped_count: 5,
                    tokens_before: 10000,
                    tokens_after: 5000,
                    overflow: false,
                },
            },
        ),
        (
            "ModelFallback",
            AgentEvent::ModelFallback {
                from_model: model.clone(),
                to_model: ModelSpec::new("openai", "gpt-4"),
            },
        ),
        (
            "ModelCycled",
            AgentEvent::ModelCycled {
                old: model.clone(),
                new: ModelSpec::new("openai", "gpt-4"),
                reason: "throttled".into(),
            },
        ),
        (
            "Custom",
            AgentEvent::Custom(Emission::new("test_event", json!({"key": "value"}))),
        ),
    ];

    for (label, event) in &events {
        let json = serde_json::to_value(event).unwrap();
        let deserialized: AgentEvent = serde_json::from_value(json.clone()).unwrap_or_else(|e| {
            panic!("Failed to deserialize AgentEvent variant '{label}': {e}")
        });
        // Compare by re-serializing — exact equality may not hold for all types.
        let reserialized = serde_json::to_value(&deserialized).unwrap();
        assert_eq!(
            json, reserialized,
            "Roundtrip mismatch for AgentEvent variant '{label}'"
        );
    }
}

#[test]
fn agent_event_deserialize_invalid_variant() {
    let json = json!({"event": "nonexistent_event"});
    let result = serde_json::from_value::<AgentEvent>(json);
    assert!(result.is_err());
}

#[test]
fn agent_event_deserialize_missing_required_field() {
    // "turn_end" requires assistant_message, tool_results, reason, and snapshot fields.
    let json = json!({"event": "turn_end"});
    let result = serde_json::from_value::<AgentEvent>(json);
    assert!(result.is_err());
}

#[test]
fn agent_event_deserialize_type_mismatch() {
    let json = json!({"event": 123});
    let result = serde_json::from_value::<AgentEvent>(json);
    assert!(result.is_err());
}
