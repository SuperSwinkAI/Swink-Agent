//! Tests for `context_version`.
#![cfg(test)]

use super::*;
use crate::context_transformer::SlidingWindowTransformer;
use crate::types::{ContentBlock, UserMessage};

fn text_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: text.to_owned(),
        }],
        timestamp: 0,
        cache_hint: None,
    }))
}

#[test]
fn versioning_captures_dropped_messages() {
    let store: Arc<dyn ContextVersionStore> = Arc::new(InMemoryVersionStore::new());
    let inner = SlidingWindowTransformer::new(250, 100, 1);
    let transformer = VersioningTransformer::new(inner, Arc::clone(&store));

    // Each message: 400 chars / 4 = 100 tokens. Budget 250, anchor 1.
    let body = "x".repeat(400);
    let mut messages = vec![
        text_message(&body),
        text_message(&body),
        text_message(&body),
        text_message(&body),
    ];

    let report = transformer.transform(&mut messages, false);
    assert!(report.is_some());

    // Messages should be compacted.
    assert_eq!(messages.len(), 2);

    // A version should have been saved.
    let versions = store.list_versions();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].version, 1);
    assert_eq!(versions[0].message_count, 2); // 2 messages were dropped

    // Load and verify.
    let v = store.load_version(1).unwrap();
    assert_eq!(v.messages.len(), 2);
    assert!(v.summary.is_none());
}

#[test]
fn versioning_with_summarizer() {
    struct TestSummarizer;
    impl ContextSummarizer for TestSummarizer {
        fn summarize(&self, messages: &[LlmMessage]) -> Option<String> {
            Some(format!("Summary of {} messages", messages.len()))
        }
    }

    let store: Arc<dyn ContextVersionStore> = Arc::new(InMemoryVersionStore::new());
    let inner = SlidingWindowTransformer::new(250, 100, 1);
    let transformer = VersioningTransformer::new(inner, Arc::clone(&store))
        .with_summarizer(Arc::new(TestSummarizer));

    let body = "x".repeat(400);
    let mut messages = vec![
        text_message(&body),
        text_message(&body),
        text_message(&body),
        text_message(&body),
    ];

    transformer.transform(&mut messages, false);

    let v = store.load_version(1).unwrap();
    assert_eq!(v.summary.as_deref(), Some("Summary of 2 messages"));
}

#[test]
fn no_compaction_no_version_saved() {
    let store: Arc<dyn ContextVersionStore> = Arc::new(InMemoryVersionStore::new());
    let inner = SlidingWindowTransformer::new(10_000, 5_000, 1);
    let transformer = VersioningTransformer::new(inner, Arc::clone(&store));

    let mut messages = vec![text_message("hello"), text_message("world")];
    let report = transformer.transform(&mut messages, false);

    assert!(report.is_none());
    assert!(store.list_versions().is_empty());
}

#[test]
fn multiple_compactions_increment_version() {
    let store: Arc<dyn ContextVersionStore> = Arc::new(InMemoryVersionStore::new());
    let inner = SlidingWindowTransformer::new(250, 100, 1);
    let transformer = VersioningTransformer::new(inner, Arc::clone(&store));

    let body = "x".repeat(400);

    // First compaction.
    let mut messages = vec![
        text_message(&body),
        text_message(&body),
        text_message(&body),
        text_message(&body),
    ];
    transformer.transform(&mut messages, false);

    // Second compaction.
    let mut messages = vec![
        text_message(&body),
        text_message(&body),
        text_message(&body),
        text_message(&body),
    ];
    transformer.transform(&mut messages, false);

    let versions = store.list_versions();
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].version, 1);
    assert_eq!(versions[1].version, 2);
}

#[test]
fn latest_version_returns_most_recent() {
    let store: Arc<dyn ContextVersionStore> = Arc::new(InMemoryVersionStore::new());
    let inner = SlidingWindowTransformer::new(250, 100, 1);
    let transformer = VersioningTransformer::new(inner, Arc::clone(&store));

    let body = "x".repeat(400);
    for _ in 0..3 {
        let mut messages = vec![
            text_message(&body),
            text_message(&body),
            text_message(&body),
            text_message(&body),
        ];
        transformer.transform(&mut messages, false);
    }

    let latest = store.latest_version().unwrap();
    assert_eq!(latest.version, 3);
}

#[test]
fn in_memory_store_load_nonexistent() {
    let store = InMemoryVersionStore::new();
    assert!(store.load_version(999).is_none());
    assert!(store.is_empty());
}

#[test]
fn version_meta_fields_correct() {
    let store: Arc<dyn ContextVersionStore> = Arc::new(InMemoryVersionStore::new());
    let inner = SlidingWindowTransformer::new(250, 100, 1);
    let transformer = VersioningTransformer::new(inner, Arc::clone(&store));

    let body = "x".repeat(400);
    let mut messages = vec![
        text_message(&body),
        text_message(&body),
        text_message(&body),
        text_message(&body),
    ];
    transformer.transform(&mut messages, false);

    let meta = &store.list_versions()[0];
    assert_eq!(meta.version, 1);
    assert_eq!(meta.turn, 1);
    assert!(!meta.has_summary);
    assert!(meta.timestamp > 0);
    assert_eq!(meta.message_count, 2);
}

#[test]
fn store_accessor() {
    let store: Arc<dyn ContextVersionStore> = Arc::new(InMemoryVersionStore::new());
    let inner = SlidingWindowTransformer::new(250, 100, 1);
    let transformer = VersioningTransformer::new(inner, Arc::clone(&store));

    // Verify store() returns the same store.
    assert!(transformer.store().list_versions().is_empty());
}

// Regression tests for #164: explicit compaction results (no Debug-string diff)

#[test]
fn report_dropped_messages_populated_by_compaction() {
    // Verify that CompactionReport.dropped_messages contains the correct
    // messages after compact_sliding_window_with runs — not reconstructed
    // via Debug-string diffing.
    use crate::context::compact_sliding_window_with;

    // Each message: 400 chars / 4 = 100 tokens.
    let body = "x".repeat(400);
    let mut messages = vec![
        text_message(&body), // anchor (100t)
        text_message(&body), // dropped (100t)
        text_message(&body), // dropped (100t)
        text_message(&body), // tail (100t)
    ];
    // Total: 400t. Budget 250 with anchor=1:
    // anchor(100t) + tail(100t) = 200t fits; middle 2 dropped.
    let report = compact_sliding_window_with(&mut messages, 250, 1, None).unwrap();

    // The middle two messages should be in dropped_messages.
    assert_eq!(report.dropped_messages.len(), 2);
    // Anchor and tail survive.
    assert_eq!(messages.len(), 2);
}

#[test]
fn versioning_uses_report_dropped_messages_not_debug_diff() {
    // Verify VersioningTransformer correctly captures dropped content
    // from the report rather than through snapshot diffing.
    let store: Arc<dyn ContextVersionStore> = Arc::new(InMemoryVersionStore::new());
    let inner = SlidingWindowTransformer::new(250, 100, 1);
    let transformer = VersioningTransformer::new(inner, Arc::clone(&store));

    let body_a = "a".repeat(400); // 100 tokens
    let body_b = "b".repeat(400); // 100 tokens

    // Messages: anchor(a), dropped(a), dropped(b), tail(b).
    // Budget 250, anchor=1: anchor(100t) + tail(100t) = 200t fits.
    // Middle 2 messages (body_a, body_b) dropped.
    let mut messages = vec![
        text_message(&body_a),
        text_message(&body_a),
        text_message(&body_b),
        text_message(&body_b),
    ];

    let report = transformer.transform(&mut messages, false);
    assert!(report.is_some());

    let v = store.load_version(1).unwrap();
    // Two middle messages were dropped.
    assert_eq!(v.messages.len(), 2);
    // Verify dropped content — if debug-string diffing were used and broke,
    // the wrong messages would be captured.
    if let LlmMessage::User(ref u) = v.messages[0] {
        if let ContentBlock::Text { ref text } = u.content[0] {
            assert_eq!(text, &body_a);
        } else {
            panic!("expected text block");
        }
    } else {
        panic!("expected user message");
    }
}

#[test]
fn custom_messages_excluded_from_dropped_messages() {
    // Custom messages should be filtered out of CompactionReport.dropped_messages
    // (they're not LlmMessage variants).
    use crate::context::compact_sliding_window_with;
    use crate::types::CustomMessage;

    #[derive(Debug)]
    struct Marker;
    impl CustomMessage for Marker {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    let body = "x".repeat(400); // 100 tokens each
    let mut messages = vec![
        text_message(&body),                    // anchor
        AgentMessage::Custom(Box::new(Marker)), // custom — dropped but excluded
        text_message(&body),                    // dropped
        text_message(&body),                    // tail
    ];
    // Budget 250, anchor=1: anchor(100t) + tail(100t) fits.
    let report = compact_sliding_window_with(&mut messages, 250, 1, None).unwrap();

    // Custom message is filtered out; only the LlmMessage is in dropped_messages.
    assert_eq!(report.dropped_messages.len(), 1);
}
