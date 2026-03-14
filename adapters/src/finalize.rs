//! Shared end-of-stream block finalization for adapter state machines.
//!
//! Every streaming adapter must close open content blocks (text, thinking,
//! tool-call) when the stream ends — whether normally, on cancellation, or on
//! error.  The [`StreamFinalize`] trait captures the adapter-specific state
//! interrogation, while [`finalize_blocks`] provides the shared event
//! generation logic.

use swink_agent::stream::AssistantMessageEvent;

// ─── OpenBlock ──────────────────────────────────────────────────────────────

/// A content block that is still open and needs a closing event.
pub enum OpenBlock {
    /// An open text block.
    Text { content_index: usize },
    /// An open thinking block.
    Thinking {
        content_index: usize,
        signature: Option<String>,
    },
    /// An open tool-call block.
    ToolCall { content_index: usize },
}

// ─── StreamFinalize trait ───────────────────────────────────────────────────

/// Drain all open content blocks from a streaming state machine.
///
/// Implementors return the currently-open blocks **in the order they should
/// be closed** (typically sorted by `content_index`). The returned blocks
/// are consumed — the state should no longer consider them open after this
/// call.
pub trait StreamFinalize {
    /// Remove and return all open blocks, ordered by content index.
    fn drain_open_blocks(&mut self) -> Vec<OpenBlock>;
}

// ─── Shared finalize function ───────────────────────────────────────────────

/// Close every open content block and return the corresponding end events.
///
/// This replaces the per-adapter `finalize_blocks` helpers with a single
/// implementation that delegates to [`StreamFinalize::drain_open_blocks`].
pub fn finalize_blocks(state: &mut impl StreamFinalize) -> Vec<AssistantMessageEvent> {
    state
        .drain_open_blocks()
        .into_iter()
        .map(|block| match block {
            OpenBlock::Text { content_index } => {
                AssistantMessageEvent::TextEnd { content_index }
            }
            OpenBlock::Thinking {
                content_index,
                signature,
            } => AssistantMessageEvent::ThinkingEnd {
                content_index,
                signature,
            },
            OpenBlock::ToolCall { content_index } => {
                AssistantMessageEvent::ToolCallEnd { content_index }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn multiple_blocks_in_order() {
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
    fn drain_is_idempotent() {
        let mut state = FakeState {
            blocks: vec![OpenBlock::Text { content_index: 0 }],
        };
        let first = finalize_blocks(&mut state);
        let second = finalize_blocks(&mut state);
        assert_eq!(first.len(), 1);
        assert!(second.is_empty());
    }
}
