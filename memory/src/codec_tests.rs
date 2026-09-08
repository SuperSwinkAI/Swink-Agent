//! Tests for `codec`.
#![cfg(test)]

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use super::*;
use swink_agent::{ContentBlock, CustomMessage, LlmMessage, UserMessage};
use tracing_subscriber::fmt::MakeWriter;

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

fn user_msg() -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(
        UserMessage::new(vec![ContentBlock::Text {
            text: "hello".to_string(),
        }])
        .with_timestamp(1),
    ))
}

#[test]
fn encode_decode_llm_roundtrip() {
    let msg = user_msg();
    let (kind, json) = encode(&msg, "test-session").expect("llm encodes");
    assert_eq!(kind, MessageKind::Llm);

    let decoded = decode(MessageKind::Llm, &json, None)
        .expect("no io error")
        .expect("decoded to Some");
    assert!(matches!(decoded, AgentMessage::Llm(LlmMessage::User(_))));
}

#[test]
fn message_kind_parse_and_as_str() {
    assert_eq!(MessageKind::parse("llm"), Some(MessageKind::Llm));
    assert_eq!(MessageKind::parse("custom"), Some(MessageKind::Custom));
    assert_eq!(MessageKind::parse("unknown"), None);
    assert_eq!(MessageKind::Llm.as_str(), "llm");
    assert_eq!(MessageKind::Custom.as_str(), "custom");
}

#[test]
fn decode_llm_errors_on_malformed_json() {
    let result = decode(MessageKind::Llm, "not-valid-json", None);
    assert!(result.is_err());
}

#[test]
fn decode_custom_returns_none_without_registry() {
    // A minimal valid custom-message envelope.
    let envelope = serde_json::json!({
        "type_name": "MyType",
        "data": {}
    });
    let json = serde_json::to_string(&envelope).unwrap();
    let result = decode(MessageKind::Custom, &json, None).expect("no io error");
    assert!(result.is_none(), "no registry → None, not an error");
}

#[test]
fn decode_jsonl_message_line_reads_tagged_message_entry() {
    let entry = SessionEntry::Message(LlmMessage::User(
        UserMessage::new(vec![ContentBlock::Text {
            text: "from-entry".to_string(),
        }])
        .with_timestamp(7),
    ));
    let line = serde_json::to_string(&entry).unwrap();

    let decoded = decode_jsonl_message_line(&line, None)
        .expect("parse succeeds")
        .expect("message preserved");
    assert!(matches!(decoded, AgentMessage::Llm(LlmMessage::User(_))));
}

#[test]
fn decode_jsonl_message_line_skips_state_records() {
    let line = serde_json::json!({
        "_state": true,
        "data": { "cursor": 42 }
    })
    .to_string();

    let decoded = decode_jsonl_message_line(&line, None).expect("parse succeeds");
    assert!(decoded.is_none());
}

#[test]
fn session_entry_codec_roundtrips_message_entries() {
    let entry = SessionEntry::Message(LlmMessage::User(
        UserMessage::new(vec![ContentBlock::Text {
            text: "hello".to_string(),
        }])
        .with_timestamp(1),
    ));

    let (kind, data) = encode_session_entry(&entry).unwrap();
    let decoded = decode_session_entry(&kind, &data)
        .expect("decode succeeds")
        .expect("entry preserved");

    assert!(matches!(
        decoded,
        SessionEntry::Message(LlmMessage::User(_))
    ));
}

#[test]
fn session_entry_codec_preserves_custom_entries_despite_custom_kind_overlap() {
    let entry = SessionEntry::Custom {
        type_name: "audit".to_string(),
        data: serde_json::json!({"ok": true}),
        timestamp: 11,
    };

    let (kind, data) = encode_session_entry(&entry).unwrap();
    let decoded = decode_session_entry(&kind, &data)
        .expect("decode succeeds")
        .expect("entry preserved");

    assert!(matches!(decoded, SessionEntry::Custom { .. }));
}

#[test]
fn encode_warning_redacts_non_serializable_custom_debug_payload() {
    let msg = AgentMessage::Custom(Box::new(NamedNonSerializableCustom {
        _secret: LEAK_SENTINEL.to_string(),
    }));

    let logs = SharedLogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .without_time()
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let encoded = encode(&msg, "session-123");
    let log_output = logs.contents();

    assert!(encoded.is_none());
    assert!(
        !log_output.contains(LEAK_SENTINEL),
        "warning log leaked debug payload: {log_output}"
    );
    assert!(
        log_output.contains("named_non_serializable"),
        "warning log should include custom type name: {log_output}"
    );
    assert!(
        log_output.contains("session-123"),
        "warning log should include session context: {log_output}"
    );
}
