//! Tests for `compaction`.
#![cfg(test)]

use super::*;
use swink_agent::{ContentBlock, LlmMessage, UserMessage};

fn text_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(
        UserMessage::new(vec![ContentBlock::Text {
            text: text.to_owned(),
        }])
        .with_timestamp(0),
    ))
}

#[test]
fn without_summary_behaves_like_sliding_window() {
    let compactor = SummarizingCompactor::new(250, 100, 1);
    let compact = compactor.compaction_fn();

    // Each message: 400 chars / 4 = 100 tokens
    let body = "x".repeat(400);
    let mut messages = vec![
        text_message(&body),
        text_message(&body),
        text_message(&body),
        text_message(&body),
    ];

    compact(&mut messages, false);

    // Same behavior as sliding_window(250, 100, 1): anchor(1) + tail(1) = 2
    assert_eq!(messages.len(), 2);
}

#[test]
fn with_summary_injects_after_anchor() {
    let compactor = SummarizingCompactor::new(250, 100, 1);
    compactor.set_summary("Earlier we discussed testing strategies.");
    let compact = compactor.compaction_fn();

    let body = "x".repeat(400);
    let mut messages = vec![
        text_message(&body),
        text_message(&body),
        text_message(&body),
        text_message(&body),
    ];

    compact(&mut messages, false);

    // anchor(1) + summary(1) + tail(1) = 3
    assert_eq!(messages.len(), 3);

    // Second message should be the summary (after anchor).
    if let AgentMessage::Llm(LlmMessage::Assistant(a)) = &messages[1] {
        let text = ContentBlock::extract_text(&a.content);
        assert!(
            text.contains("[Context summary"),
            "expected summary prefix, got: {text}"
        );
        assert!(text.contains("testing strategies"));
    } else {
        panic!("expected assistant message at index 1");
    }
}

#[test]
fn no_compaction_needed_no_summary_injected() {
    let compactor = SummarizingCompactor::new(10_000, 5_000, 1);
    compactor.set_summary("This should not appear.");
    let compact = compactor.compaction_fn();

    let mut messages = vec![text_message("hello"), text_message("world")];
    compact(&mut messages, false);

    // Under budget, no compaction, no summary injection.
    assert_eq!(messages.len(), 2);
}

#[test]
fn clear_summary_removes_stored_text() {
    let compactor = SummarizingCompactor::new(100, 50, 1);
    compactor.set_summary("some summary");
    assert!(compactor.has_summary());

    compactor.clear_summary();
    assert!(!compactor.has_summary());
}

#[test]
fn summary_injected_as_assistant_message() {
    let compactor = SummarizingCompactor::new(250, 100, 1);
    compactor.set_summary("Key discussion points here.");
    let compact = compactor.compaction_fn();

    let body = "x".repeat(400);
    let mut messages = vec![
        text_message(&body),
        text_message(&body),
        text_message(&body),
    ];

    compact(&mut messages, false);

    // Find the summary message
    let summary_msg = messages.iter().find(|m| {
        if let AgentMessage::Llm(LlmMessage::Assistant(a)) = m {
            ContentBlock::extract_text(&a.content).contains("[Context summary")
        } else {
            false
        }
    });
    assert!(
        summary_msg.is_some(),
        "summary should be an AssistantMessage"
    );
}

#[test]
fn compaction_with_single_message_returns_unchanged() {
    let compactor = SummarizingCompactor::new(10_000, 5_000, 1);
    let compact = compactor.compaction_fn();

    let mut messages = vec![text_message("single message")];
    compact(&mut messages, false);

    assert_eq!(messages.len(), 1);
}

#[test]
fn summary_consumed_after_injection() {
    let compactor = SummarizingCompactor::new(250, 100, 1);
    compactor.set_summary("Consumed summary.");
    let compact = compactor.compaction_fn();

    let body = "x".repeat(400);
    let mut messages = vec![
        text_message(&body),
        text_message(&body),
        text_message(&body),
        text_message(&body),
    ];

    compact(&mut messages, false);

    // Summary should have been consumed
    assert!(
        !compactor.has_summary(),
        "summary should be consumed after injection"
    );
}
