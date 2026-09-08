//! Tests for `context_transformer`.
#![cfg(test)]

use super::*;
use crate::types::{ContentBlock, LlmMessage, UserMessage};

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
fn sliding_window_transformer_reports_dropped_messages() {
    let transformer = SlidingWindowTransformer::new(250, 100, 1);
    // Each message: 400 chars / 4 = 100 tokens
    let body = "x".repeat(400);
    let mut messages = vec![
        text_message(&body),
        text_message(&body),
        text_message(&body),
        text_message(&body),
    ];

    let report = transformer.transform(&mut messages, false);
    assert!(report.is_some(), "should report compaction");
    let report = report.unwrap();
    assert_eq!(report.dropped_count, 2);
    assert_eq!(report.tokens_before, 400);
    assert!(report.tokens_after < report.tokens_before);
    assert!(!report.overflow);
    assert_eq!(messages.len(), 2);
}

#[test]
fn sliding_window_transformer_no_report_under_budget() {
    let transformer = SlidingWindowTransformer::new(10_000, 5_000, 1);
    let mut messages = vec![text_message("hello"), text_message("world")];

    let report = transformer.transform(&mut messages, false);
    assert!(report.is_none(), "should not report when under budget");
    assert_eq!(messages.len(), 2);
}

#[test]
fn closure_blanket_impl_works() {
    let closure = |msgs: &mut Vec<AgentMessage>, _overflow: bool| {
        if msgs.len() > 2 {
            msgs.truncate(2);
        }
    };

    let mut messages = vec![
        text_message("a"),
        text_message("b"),
        text_message("c"),
        text_message("d"),
    ];

    let report = closure.transform(&mut messages, false);
    assert!(report.is_some());
    let report = report.unwrap();
    assert_eq!(report.dropped_count, 2);
    // Bare closures can't report token counts
    assert_eq!(report.tokens_before, 0);
    assert_eq!(report.tokens_after, 0);
    assert_eq!(messages.len(), 2);
}

#[test]
fn overflow_uses_smaller_budget() {
    let transformer = SlidingWindowTransformer::new(1000, 150, 1);
    let body = "x".repeat(400);
    let mut messages = vec![
        text_message(&body),
        text_message(&body),
        text_message(&body),
        text_message(&body),
    ];

    // Under normal budget (1000), total is 400 tokens -- no trim.
    let report = transformer.transform(&mut messages, false);
    assert!(report.is_none());
    assert_eq!(messages.len(), 4);

    // Under overflow budget (150), should trim.
    let report = transformer.transform(&mut messages, true);
    assert!(report.is_some());
    let report = report.unwrap();
    assert!(report.overflow);
    assert!(messages.len() < 4);
}

#[test]
fn sliding_window_transformer_with_custom_counter() {
    use crate::context::TokenCounter;

    /// Counts every character as one token (4x the default heuristic).
    struct CharCounter;

    impl TokenCounter for CharCounter {
        fn count_tokens(&self, message: &AgentMessage) -> usize {
            match message {
                AgentMessage::Llm(llm) => {
                    let blocks = match llm {
                        LlmMessage::User(m) => &m.content,
                        _ => return 0,
                    };
                    blocks
                        .iter()
                        .map(|b| match b {
                            ContentBlock::Text { text } => text.len(),
                            _ => 0,
                        })
                        .sum()
                }
                AgentMessage::Custom(_) => 50,
            }
        }
    }

    // Each message: 400 chars.
    // Default counter: 400/4 = 100 tokens each.
    // CharCounter: 400 tokens each.
    let body = "x".repeat(400);

    // With default counter, 4 * 100 = 400 tokens. Budget 500 => no trim.
    let default_transformer = SlidingWindowTransformer::new(500, 250, 1);
    let mut messages = vec![
        text_message(&body),
        text_message(&body),
        text_message(&body),
        text_message(&body),
    ];
    let report = default_transformer.transform(&mut messages, false);
    assert!(
        report.is_none(),
        "default counter should not trim at budget 500"
    );
    assert_eq!(messages.len(), 4);

    // With CharCounter, 4 * 400 = 1600 tokens. Budget 500 => trims.
    let custom_transformer =
        SlidingWindowTransformer::new(500, 250, 1).with_token_counter(Arc::new(CharCounter));
    let mut messages = vec![
        text_message(&body),
        text_message(&body),
        text_message(&body),
        text_message(&body),
    ];
    let report = custom_transformer.transform(&mut messages, false);
    assert!(report.is_some(), "char counter should trim at budget 500");
    assert!(messages.len() < 4);
}
