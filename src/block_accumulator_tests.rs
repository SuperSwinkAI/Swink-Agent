//! Tests for `block_accumulator`.
#![cfg(test)]

use super::*;

// ── finalize_blocks tests ──────────────────────────────────────────────

struct FakeState {
    blocks: Vec<OpenBlock>,
}

impl StreamFinalize for FakeState {
    fn drain_open_blocks(&mut self) -> Vec<OpenBlock> {
        std::mem::take(&mut self.blocks)
    }
}

#[test]
fn empty_state_produces_no_events() {
    let mut state = FakeState { blocks: vec![] };
    let events = finalize_blocks(&mut state);
    assert!(events.is_empty());
}

#[test]
fn text_block_emits_text_end() {
    let mut state = FakeState {
        blocks: vec![OpenBlock::Text { content_index: 0 }],
    };
    let events = finalize_blocks(&mut state);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::TextEnd { content_index: 0 }
    ));
}

#[test]
fn thinking_block_emits_thinking_end() {
    let mut state = FakeState {
        blocks: vec![OpenBlock::Thinking {
            content_index: 1,
            signature: Some("sig".to_string()),
        }],
    };
    let events = finalize_blocks(&mut state);
    assert_eq!(events.len(), 1);
    match &events[0] {
        AssistantMessageEvent::ThinkingEnd {
            content_index,
            signature,
        } => {
            assert_eq!(*content_index, 1);
            assert_eq!(signature.as_deref(), Some("sig"));
        }
        other => panic!("expected ThinkingEnd, got {other:?}"),
    }
}

#[test]
fn tool_call_block_emits_tool_call_end() {
    let mut state = FakeState {
        blocks: vec![OpenBlock::ToolCall { content_index: 2 }],
    };
    let events = finalize_blocks(&mut state);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::ToolCallEnd { content_index: 2 }
    ));
}

#[test]
fn multiple_finalize_blocks_in_order() {
    let mut state = FakeState {
        blocks: vec![
            OpenBlock::Thinking {
                content_index: 0,
                signature: None,
            },
            OpenBlock::Text { content_index: 1 },
            OpenBlock::ToolCall { content_index: 2 },
            OpenBlock::ToolCall { content_index: 3 },
        ],
    };
    let events = finalize_blocks(&mut state);
    assert_eq!(events.len(), 4);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::ThinkingEnd {
            content_index: 0,
            ..
        }
    ));
    assert!(matches!(
        events[1],
        AssistantMessageEvent::TextEnd { content_index: 1 }
    ));
    assert!(matches!(
        events[2],
        AssistantMessageEvent::ToolCallEnd { content_index: 2 }
    ));
    assert!(matches!(
        events[3],
        AssistantMessageEvent::ToolCallEnd { content_index: 3 }
    ));
}

#[test]
fn finalize_drain_is_idempotent() {
    let mut state = FakeState {
        blocks: vec![OpenBlock::Text { content_index: 0 }],
    };
    let first = finalize_blocks(&mut state);
    let second = finalize_blocks(&mut state);
    assert_eq!(first.len(), 1);
    assert!(second.is_empty());
}

// ── BlockAccumulator tests ─────────────────────────────────────────────

#[test]
fn initial_state_is_empty() {
    let acc = BlockAccumulator::new();
    assert!(!acc.text_open());
    assert!(!acc.thinking_open());
    assert_eq!(acc.text_index(), None);
    assert_eq!(acc.thinking_index(), None);
}

#[test]
fn text_block_lifecycle() {
    let mut acc = BlockAccumulator::new();

    let start = acc.ensure_text_open();
    assert!(matches!(
        start,
        Some(AssistantMessageEvent::TextStart { content_index: 0 })
    ));
    assert!(acc.text_open());
    assert_eq!(acc.text_index(), Some(0));

    // Second call is a no-op
    assert!(acc.ensure_text_open().is_none());

    let delta = acc.text_delta("hello".to_string());
    assert!(matches!(
        delta,
        Some(AssistantMessageEvent::TextDelta {
            content_index: 0,
            ..
        })
    ));

    let end = acc.close_text();
    assert!(matches!(
        end,
        Some(AssistantMessageEvent::TextEnd { content_index: 0 })
    ));
    assert!(!acc.text_open());

    // Double-close is a no-op
    assert!(acc.close_text().is_none());
}

#[test]
fn thinking_block_lifecycle() {
    let mut acc = BlockAccumulator::new();

    let start = acc.ensure_thinking_open();
    assert!(matches!(
        start,
        Some(AssistantMessageEvent::ThinkingStart { content_index: 0 })
    ));
    assert!(acc.thinking_open());

    let delta = acc.thinking_delta("thought".to_string());
    assert!(matches!(
        delta,
        Some(AssistantMessageEvent::ThinkingDelta {
            content_index: 0,
            ..
        })
    ));

    let end = acc.close_thinking(Some("sig".to_string()));
    match end {
        Some(AssistantMessageEvent::ThinkingEnd {
            content_index: 0,
            signature,
        }) => {
            assert_eq!(signature.as_deref(), Some("sig"));
        }
        other => panic!("expected ThinkingEnd, got {other:?}"),
    }
    assert!(!acc.thinking_open());
}

#[test]
fn accumulated_signature_used_when_close_has_none() {
    let mut acc = BlockAccumulator::new();
    acc.ensure_thinking_open();
    acc.set_thinking_signature("early-sig".to_string());

    let end = acc.close_thinking(None);
    match end {
        Some(AssistantMessageEvent::ThinkingEnd { signature, .. }) => {
            assert_eq!(signature.as_deref(), Some("early-sig"));
        }
        other => panic!("expected ThinkingEnd, got {other:?}"),
    }
}

#[test]
fn close_signature_overrides_accumulated() {
    let mut acc = BlockAccumulator::new();
    acc.ensure_thinking_open();
    acc.set_thinking_signature("early-sig".to_string());

    let end = acc.close_thinking(Some("late-sig".to_string()));
    match end {
        Some(AssistantMessageEvent::ThinkingEnd { signature, .. }) => {
            assert_eq!(signature.as_deref(), Some("late-sig"));
        }
        other => panic!("expected ThinkingEnd, got {other:?}"),
    }
}

#[test]
fn tool_call_lifecycle() {
    let mut acc = BlockAccumulator::new();

    let (ci, start) = acc.open_tool_call("id-1".to_string(), "my_tool".to_string());
    assert_eq!(ci, 0);
    assert!(matches!(
        start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            ..
        }
    ));

    let delta = BlockAccumulator::tool_call_delta(ci, r#"{"x":1}"#.to_string());
    assert!(matches!(
        delta,
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            ..
        }
    ));

    let end = acc.close_tool_call(ci);
    assert!(matches!(
        end,
        Some(AssistantMessageEvent::ToolCallEnd { content_index: 0 })
    ));

    // Close again → None
    assert!(acc.close_tool_call(ci).is_none());
}

#[test]
fn indices_are_monotonically_allocated() {
    let mut acc = BlockAccumulator::new();
    acc.ensure_text_open(); // index 0
    acc.close_text();
    let (ci1, _) = acc.open_tool_call("id".to_string(), "t".to_string()); // index 1
    acc.close_tool_call(ci1);
    acc.ensure_thinking_open(); // index 2
    assert_eq!(acc.thinking_index(), Some(2));
}

#[test]
fn drain_produces_sorted_close_events() {
    let mut acc = BlockAccumulator::new();
    acc.ensure_thinking_open(); // index 0
    acc.ensure_text_open(); // index 1
    acc.open_tool_call("id-a".to_string(), "a".to_string()); // index 2
    acc.open_tool_call("id-b".to_string(), "b".to_string()); // index 3

    let events = finalize_blocks(&mut acc);
    assert_eq!(events.len(), 4);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::ThinkingEnd {
            content_index: 0,
            ..
        }
    ));
    assert!(matches!(
        events[1],
        AssistantMessageEvent::TextEnd { content_index: 1 }
    ));
    assert!(matches!(
        events[2],
        AssistantMessageEvent::ToolCallEnd { content_index: 2 }
    ));
    assert!(matches!(
        events[3],
        AssistantMessageEvent::ToolCallEnd { content_index: 3 }
    ));
}

#[test]
fn accumulator_drain_is_idempotent() {
    let mut acc = BlockAccumulator::new();
    acc.ensure_text_open();

    let first = finalize_blocks(&mut acc);
    let second = finalize_blocks(&mut acc);
    assert_eq!(first.len(), 1);
    assert!(second.is_empty());
}

#[test]
fn mixed_text_and_thinking_stream() {
    let mut acc = BlockAccumulator::new();

    // Thinking comes first
    let thinking_start = acc.ensure_thinking_open().unwrap();
    assert!(matches!(
        thinking_start,
        AssistantMessageEvent::ThinkingStart { content_index: 0 }
    ));

    let thinking_end = acc.close_thinking(None).unwrap();
    assert!(matches!(
        thinking_end,
        AssistantMessageEvent::ThinkingEnd {
            content_index: 0,
            ..
        }
    ));

    // Text follows
    let text_start = acc.ensure_text_open().unwrap();
    assert!(matches!(
        text_start,
        AssistantMessageEvent::TextStart { content_index: 1 }
    ));

    let text_end = acc.close_text().unwrap();
    assert!(matches!(
        text_end,
        AssistantMessageEvent::TextEnd { content_index: 1 }
    ));
}

#[test]
fn tool_calls_in_drain_are_sorted_by_content_index() {
    let mut acc = BlockAccumulator::new();
    let (ci_a, _) = acc.open_tool_call("a".to_string(), "tool_a".to_string());
    let (ci_b, _) = acc.open_tool_call("b".to_string(), "tool_b".to_string());
    // Close b first to scramble internal vec order
    acc.close_tool_call(ci_b);
    // Re-open another tool call
    let (ci_c, _) = acc.open_tool_call("c".to_string(), "tool_c".to_string());

    // a and c are still open
    let events = finalize_blocks(&mut acc);
    assert_eq!(events.len(), 2);
    assert!(
        matches!(events[0], AssistantMessageEvent::ToolCallEnd { content_index } if content_index == ci_a)
    );
    assert!(
        matches!(events[1], AssistantMessageEvent::ToolCallEnd { content_index } if content_index == ci_c)
    );
}
