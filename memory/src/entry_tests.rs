//! Tests for `entry`.
#![cfg(test)]

use super::*;
use swink_agent::{ContentBlock, ModelSpec, UserMessage};

#[test]
fn session_entry_serde_roundtrip_message() {
    let entry = SessionEntry::Message(LlmMessage::User(
        UserMessage::new(vec![ContentBlock::Text {
            text: "hello".to_string(),
        }])
        .with_timestamp(42),
    ));

    let json = serde_json::to_string(&entry).unwrap();
    let parsed: SessionEntry = serde_json::from_str(&json).unwrap();
    assert!(matches!(parsed, SessionEntry::Message(LlmMessage::User(_))));
}

#[test]
fn session_entry_serde_roundtrip_model_change() {
    let entry = SessionEntry::ModelChange {
        from: ModelSpec::new("openai", "gpt-4"),
        to: ModelSpec::new("anthropic", "claude-3"),
        timestamp: 100,
    };

    let json = serde_json::to_string(&entry).unwrap();
    let parsed: SessionEntry = serde_json::from_str(&json).unwrap();
    assert!(matches!(
        parsed,
        SessionEntry::ModelChange { timestamp: 100, .. }
    ));
}

#[test]
fn session_entry_serde_roundtrip_compaction() {
    let entry = SessionEntry::Compaction {
        dropped_count: 15,
        tokens_before: 5000,
        tokens_after: 2000,
        timestamp: 200,
    };

    let json = serde_json::to_string(&entry).unwrap();
    let parsed: SessionEntry = serde_json::from_str(&json).unwrap();
    assert!(matches!(
        parsed,
        SessionEntry::Compaction {
            dropped_count: 15,
            tokens_before: 5000,
            tokens_after: 2000,
            timestamp: 200,
        }
    ));
}

#[test]
fn session_entry_serde_roundtrip_label() {
    let entry = SessionEntry::Label {
        text: "important".to_string(),
        message_index: 5,
        timestamp: 300,
    };

    let json = serde_json::to_string(&entry).unwrap();
    let parsed: SessionEntry = serde_json::from_str(&json).unwrap();
    assert!(matches!(
        parsed,
        SessionEntry::Label {
            message_index: 5,
            timestamp: 300,
            ..
        }
    ));
}

#[test]
fn session_entry_serde_roundtrip_custom() {
    let entry = SessionEntry::Custom {
        type_name: "my_event".to_string(),
        data: serde_json::json!({"key": "value"}),
        timestamp: 400,
    };

    let json = serde_json::to_string(&entry).unwrap();
    let parsed: SessionEntry = serde_json::from_str(&json).unwrap();
    assert!(matches!(
        parsed,
        SessionEntry::Custom { timestamp: 400, .. }
    ));
}

#[test]
fn session_entry_serde_roundtrip_thinking_level_change() {
    let entry = SessionEntry::ThinkingLevelChange {
        from: "low".to_string(),
        to: "high".to_string(),
        timestamp: 500,
    };

    let json = serde_json::to_string(&entry).unwrap();
    let parsed: SessionEntry = serde_json::from_str(&json).unwrap();
    assert!(matches!(
        parsed,
        SessionEntry::ThinkingLevelChange { timestamp: 500, .. }
    ));
}

#[test]
fn parse_old_format_as_message() {
    // Old-format: raw LlmMessage without entry_type
    let old_line = serde_json::to_string(&LlmMessage::User(
        UserMessage::new(vec![ContentBlock::Text {
            text: "old format".to_string(),
        }])
        .with_timestamp(0),
    ))
    .unwrap();

    let entry = SessionEntry::parse(&old_line).unwrap();
    assert!(matches!(entry, SessionEntry::Message(LlmMessage::User(_))));
}

#[test]
fn parse_tagged_format() {
    let tagged =
        r#"{"entry_type":"label","data":{"text":"bookmark","message_index":3,"timestamp":100}}"#;
    let entry = SessionEntry::parse(tagged).unwrap();
    assert!(matches!(entry, SessionEntry::Label { .. }));
}

#[test]
fn rich_entries_excluded_from_llm_context() {
    let entries = vec![
        SessionEntry::Message(LlmMessage::User(
            UserMessage::new(vec![ContentBlock::Text {
                text: "hello".to_string(),
            }])
            .with_timestamp(0),
        )),
        SessionEntry::ModelChange {
            from: ModelSpec::new("test", "test"),
            to: ModelSpec::new("test", "test"),
            timestamp: 1,
        },
        SessionEntry::Label {
            text: "important".to_string(),
            message_index: 0,
            timestamp: 2,
        },
        SessionEntry::Compaction {
            dropped_count: 5,
            tokens_before: 1000,
            tokens_after: 500,
            timestamp: 3,
        },
        SessionEntry::Custom {
            type_name: "test".to_string(),
            data: serde_json::json!({}),
            timestamp: 4,
        },
        SessionEntry::Message(LlmMessage::User(
            UserMessage::new(vec![ContentBlock::Text {
                text: "world".to_string(),
            }])
            .with_timestamp(5),
        )),
    ];

    let messages = SessionEntry::messages(&entries);
    assert_eq!(messages.len(), 2);
    // Only Message variants returned
    for msg in &messages {
        assert!(matches!(msg, LlmMessage::User(_)));
    }
}
