//! Tests for `message_codec`.
#![cfg(test)]

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use super::*;
use crate::types::{
    AssistantMessage, ContentBlock, Cost, CustomMessage, StopReason, Usage, UserMessage,
};
use tracing_subscriber::fmt::MakeWriter;

// ── Test helpers ────────────────────────────────────────────────────────

#[derive(Debug)]
struct NonSerializableCustom;

impl CustomMessage for NonSerializableCustom {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

const LEAK_SENTINEL: &str = "LEAK_SENTINEL_717";

#[derive(Debug)]
struct NamedNonSerializableCustom {
    _secret: String,
}

impl CustomMessage for NamedNonSerializableCustom {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn type_name(&self) -> Option<&str> {
        Some("named_non_serializable")
    }
}

#[derive(Clone, Default)]
struct SharedLogBuffer(Arc<Mutex<Vec<u8>>>);

impl SharedLogBuffer {
    fn contents(&self) -> String {
        let bytes = self.0.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }
}

struct SharedLogWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SharedLogBuffer {
    type Writer = SharedLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SharedLogWriter(Arc::clone(&self.0))
    }
}

#[derive(Debug, Clone, PartialEq)]
struct TaggedCustom {
    tag: String,
}

impl CustomMessage for TaggedCustom {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn type_name(&self) -> Option<&str> {
        Some("TaggedCustom")
    }
    fn to_json(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({ "tag": self.tag }))
    }
}

fn tagged_registry() -> CustomMessageRegistry {
    let mut reg = CustomMessageRegistry::new();
    reg.register(
        "TaggedCustom",
        Box::new(|val: serde_json::Value| {
            let tag = val
                .get("tag")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing tag".to_string())?;
            Ok(Box::new(TaggedCustom {
                tag: tag.to_string(),
            }) as Box<dyn CustomMessage>)
        }),
    );
    reg
}

fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        timestamp: 0,
        cache_hint: None,
    }))
}

fn assistant_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        provider: "test".to_string(),
        model_id: "m".to_string(),
        usage: Usage::default(),
        cost: Cost::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        error_kind: None,
        timestamp: 0,
        cache_hint: None,
    }))
}

fn custom_msg(tag: &str) -> AgentMessage {
    AgentMessage::Custom(Box::new(TaggedCustom {
        tag: tag.to_string(),
    }))
}

fn message_label(msg: &AgentMessage) -> String {
    match msg {
        AgentMessage::Llm(LlmMessage::User(u)) => {
            format!("user:{}", ContentBlock::extract_text(&u.content))
        }
        AgentMessage::Llm(LlmMessage::Assistant(a)) => {
            format!("assistant:{}", ContentBlock::extract_text(&a.content))
        }
        AgentMessage::Custom(c) => {
            if let Some(json) = c.to_json() {
                format!("custom:{}", json["tag"].as_str().unwrap_or("?"))
            } else {
                "custom:?".to_string()
            }
        }
        AgentMessage::Llm(LlmMessage::ToolResult(_)) => "other".to_string(),
    }
}

// ── serialize_messages ──────────────────────────────────────────────────

#[test]
fn serialize_skips_non_serializable_custom() {
    let messages = vec![
        user_msg("hi"),
        AgentMessage::Custom(Box::new(NonSerializableCustom)),
        assistant_msg("hello"),
    ];

    let result = serialize_messages(&messages, "test");
    assert_eq!(result.llm_messages.len(), 2);
    assert!(result.custom_messages.is_empty());
    assert_eq!(result.message_order.len(), 2);
}

#[test]
fn serialize_warning_redacts_non_serializable_custom_debug_payload() {
    let messages = vec![AgentMessage::Custom(Box::new(NamedNonSerializableCustom {
        _secret: LEAK_SENTINEL.to_string(),
    }))];

    let logs = SharedLogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .without_time()
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let result = serialize_messages(&messages, "checkpoint");
    let log_output = logs.contents();

    assert!(result.custom_messages.is_empty());
    assert!(
        !log_output.contains(LEAK_SENTINEL),
        "warning log leaked debug payload: {log_output}"
    );
    assert!(
        log_output.contains("named_non_serializable"),
        "warning log should include custom type name: {log_output}"
    );
    assert!(
        log_output.contains("checkpoint"),
        "warning log should include serialization context: {log_output}"
    );
}

#[test]
fn serialize_preserves_interleaved_order() {
    let messages = vec![
        user_msg("hello"),
        custom_msg("A"),
        assistant_msg("hi"),
        custom_msg("B"),
        user_msg("thanks"),
    ];

    let result = serialize_messages(&messages, "test");
    assert_eq!(result.llm_messages.len(), 3);
    assert_eq!(result.custom_messages.len(), 2);
    assert_eq!(result.message_order.len(), 5);

    // Verify envelope content
    assert_eq!(result.custom_messages[0]["type"], "TaggedCustom");
    assert_eq!(result.custom_messages[0]["data"]["tag"], "A");
    assert_eq!(result.custom_messages[1]["data"]["tag"], "B");
}

// ── restore_messages ───────────────────────────────────────────────────

#[test]
fn roundtrip_preserves_order() {
    let registry = tagged_registry();
    let messages = vec![
        user_msg("hello"),
        custom_msg("A"),
        assistant_msg("hi"),
        custom_msg("B"),
        user_msg("thanks"),
    ];

    let serialized = serialize_messages(&messages, "test");
    let restored = restore_messages(
        &serialized.llm_messages,
        &serialized.custom_messages,
        &serialized.message_order,
        Some(&registry),
        "test",
    );

    let labels: Vec<String> = restored.iter().map(message_label).collect();
    assert_eq!(
        labels,
        vec![
            "user:hello",
            "custom:A",
            "assistant:hi",
            "custom:B",
            "user:thanks",
        ]
    );
}

#[test]
fn restore_without_registry_skips_custom() {
    let messages = vec![user_msg("hi"), custom_msg("skipped"), assistant_msg("ok")];

    let serialized = serialize_messages(&messages, "test");
    let restored = restore_messages(
        &serialized.llm_messages,
        &serialized.custom_messages,
        &serialized.message_order,
        None,
        "test",
    );

    assert_eq!(restored.len(), 2);
    let labels: Vec<String> = restored.iter().map(message_label).collect();
    assert_eq!(labels, vec!["user:hi", "assistant:ok"]);
}

#[test]
fn legacy_fallback_no_ordering() {
    let registry = tagged_registry();
    let llm = vec![LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: "hi".to_string(),
        }],
        timestamp: 0,
        cache_hint: None,
    })];
    let custom = vec![serde_json::json!({
        "type": "TaggedCustom",
        "data": { "tag": "legacy" }
    })];

    let restored = restore_messages(&llm, &custom, &[], Some(&registry), "test");
    assert_eq!(restored.len(), 2);
    let labels: Vec<String> = restored.iter().map(message_label).collect();
    assert_eq!(labels, vec!["user:hi", "custom:legacy"]);
}

// ── restore_single_custom ──────────────────────────────────────────────

#[test]
fn restore_single_custom_with_registry() {
    let registry = tagged_registry();
    let envelope = serde_json::json!({
        "type": "TaggedCustom",
        "data": { "tag": "single" }
    });

    let result = restore_single_custom(Some(&registry), &envelope).unwrap();
    assert!(result.is_some());
    let custom = result.unwrap();
    assert_eq!(custom.type_name(), Some("TaggedCustom"));
}

#[test]
fn restore_single_custom_without_registry() {
    let envelope = serde_json::json!({ "type": "X", "data": {} });
    let result = restore_single_custom(None, &envelope).unwrap();
    assert!(result.is_none());
}

// ── SerializedCustomMessage ────────────────────────────────────────────

#[test]
fn serialized_custom_message_from_custom() {
    let original = TaggedCustom {
        tag: "hello".to_string(),
    };
    let snapshot = SerializedCustomMessage::from_custom(&original).unwrap();
    assert_eq!(snapshot.type_name(), Some("TaggedCustom"));
    assert_eq!(snapshot.to_json().unwrap()["tag"], "hello");
}

#[test]
fn serialized_custom_message_from_non_serializable() {
    let bare = NonSerializableCustom;
    assert!(SerializedCustomMessage::from_custom(&bare).is_none());
}

// ── clone_messages_for_send ────────────────────────────────────────────

#[test]
fn clone_for_send_preserves_all_serializable() {
    let messages = vec![
        user_msg("hello"),
        custom_msg("kept"),
        AgentMessage::Custom(Box::new(NonSerializableCustom)),
        assistant_msg("world"),
    ];

    let cloned = clone_messages_for_send(&messages);
    assert_eq!(cloned.len(), 3); // non-serializable custom dropped
    let labels: Vec<String> = cloned.iter().map(message_label).collect();
    assert_eq!(labels, vec!["user:hello", "custom:kept", "assistant:world"]);
}

#[test]
fn clone_for_send_custom_roundtrips_through_registry() {
    let registry = tagged_registry();
    let messages = vec![custom_msg("roundtrip")];
    let cloned = clone_messages_for_send(&messages);
    assert_eq!(cloned.len(), 1);

    // The cloned custom message can be serialized and restored
    let envelope =
        serialize_custom_message(cloned[0].downcast_ref::<SerializedCustomMessage>().unwrap())
            .unwrap();
    let restored = deserialize_custom_message(&registry, &envelope).unwrap();
    assert_eq!(
        restored
            .as_any()
            .downcast_ref::<TaggedCustom>()
            .unwrap()
            .tag,
        "roundtrip"
    );
}

// ── AgentMessage::try_clone / AgentContext ──────────────────────────────

#[test]
fn try_clone_llm_and_serializable_custom_succeed() {
    assert!(user_msg("hello").try_clone().is_some());
    // TaggedCustom has no clone_box but supports serialization — clones
    // through the SerializedCustomMessage snapshot fallback.
    let cloned = custom_msg("kept").try_clone().unwrap();
    assert!(
        cloned
            .downcast_ref::<SerializedCustomMessage>()
            .unwrap()
            .type_name()
            .is_some()
    );
}

#[test]
fn try_clone_non_serializable_custom_returns_none() {
    let msg = AgentMessage::Custom(Box::new(NonSerializableCustom));
    assert!(msg.try_clone().is_none());
}

#[test]
fn context_try_clone_is_all_or_nothing() {
    let ok = crate::types::AgentContext::new(
        "system",
        vec![user_msg("hello"), custom_msg("kept")],
        vec![],
    );
    assert_eq!(ok.try_clone().unwrap().messages.len(), 2);

    let bad = crate::types::AgentContext::new(
        "system",
        vec![
            user_msg("hello"),
            AgentMessage::Custom(Box::new(NonSerializableCustom)),
        ],
        vec![],
    );
    assert!(bad.try_clone().is_none());
}

#[test]
fn context_clone_for_send_drops_only_non_serializable() {
    let context = crate::types::AgentContext::new(
        "system",
        vec![
            user_msg("hello"),
            AgentMessage::Custom(Box::new(NonSerializableCustom)),
            custom_msg("kept"),
        ],
        vec![],
    );
    let snapshot = context.clone_for_send();
    assert_eq!(snapshot.system_prompt, "system");
    assert_eq!(snapshot.messages.len(), 2);
}
