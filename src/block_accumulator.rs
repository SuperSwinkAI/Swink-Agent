//! Shared incremental block accumulator for streaming event assembly.
//!
//! Every streaming adapter / local inference backend manages the same lifecycle:
//!
//! 1. **Allocate** a content index for each new block.
//! 2. **Open** a text, thinking, or tool-call block and emit a `*Start` event.
//! 3. **Emit** `*Delta` events as the provider sends incremental data.
//! 4. **Close** the block with a `*End` event on the provider's explicit close
//!    signal, or let [`StreamFinalize`] drain any blocks left open when the
//!    stream terminates.
//!
//! [`BlockAccumulator`] owns this state so adapters don't have to replicate
//! it. Provider-specific parsing (wire format, stop-reason mapping, usage
//! extraction) stays where it is — only the event-assembly state machine moves
//! here.

use crate::stream::AssistantMessageEvent;

// ─── OpenBlock ─────────────────────────────────────────────────────────────

/// A content block that is still open and needs a closing event.
#[derive(Debug)]
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

// ─── StreamFinalize trait ──────────────────────────────────────────────────

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

// ─── Shared finalize function ──────────────────────────────────────────────

/// Close every open content block and return the corresponding end events.
///
/// This replaces the per-adapter `finalize_blocks` helpers with a single
/// implementation that delegates to [`StreamFinalize::drain_open_blocks`].
pub fn finalize_blocks(state: &mut impl StreamFinalize) -> Vec<AssistantMessageEvent> {
    state
        .drain_open_blocks()
        .into_iter()
        .map(|block| match block {
            OpenBlock::Text { content_index } => AssistantMessageEvent::TextEnd { content_index },
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

// ─── BlockAccumulator ──────────────────────────────────────────────────────

/// Stateful accumulator for streaming content-block lifecycle events.
///
/// # Content index allocation
///
/// Each block is assigned a monotonically increasing *harness* content index
/// that is independent of any provider-side block numbering. Call
/// [`ensure_text_open`](Self::ensure_text_open),
/// [`ensure_thinking_open`](Self::ensure_thinking_open), or
/// [`open_tool_call`](Self::open_tool_call) — each allocates the next index
/// automatically.
///
/// # Draining on stream end
///
/// The accumulator implements [`StreamFinalize`]: call
/// [`finalize_blocks`] to close every block that the provider left open.
#[derive(Debug, Default)]
pub struct BlockAccumulator {
    /// Next content index to hand out.
    next_index: usize,
    /// Content index of the currently-open text block, if any.
    text_index: Option<usize>,
    /// Content index and optional signature of the currently-open thinking
    /// block, if any.
    thinking_index: Option<(usize, Option<String>)>,
    /// Content indices of all currently-open tool-call blocks, in insertion
    /// order. Sorted by content index because indices are allocated
    /// monotonically.
    open_tool_calls: Vec<usize>,
}

#[allow(clippy::missing_const_for_fn)]
impl BlockAccumulator {
    /// Create a new accumulator starting at content index 0.
    #[allow(dead_code)]
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    // ── Index allocation ───────────────────────────────────────────────────

    /// Allocate the next content index *without* opening any block.
    ///
    /// Useful when a provider sends a block-start signal that includes data
    /// requiring the caller to decide whether to open a harness block at all
    /// (e.g. Anthropic's index-remapping on thinking blocks).
    pub fn alloc_index(&mut self) -> usize {
        let idx = self.next_index;
        self.next_index += 1;
        idx
    }

    // ── Text block ─────────────────────────────────────────────────────────

    /// Ensure a text block is open, allocating a new index if one is not
    /// already open.
    ///
    /// Returns a `TextStart` event when a new block is opened, or `None` if
    /// the block was already open.
    pub fn ensure_text_open(&mut self) -> Option<AssistantMessageEvent> {
        if self.text_index.is_none() {
            let idx = self.alloc_index();
            self.text_index = Some(idx);
            Some(AssistantMessageEvent::TextStart { content_index: idx })
        } else {
            None
        }
    }

    /// Return `true` if a text block is currently open.
    #[allow(dead_code)]
    #[inline]
    pub fn text_open(&self) -> bool {
        self.text_index.is_some()
    }

    /// Return the content index of the open text block, or `None` if no text
    /// block is open.
    #[allow(dead_code)]
    #[inline]
    pub fn text_index(&self) -> Option<usize> {
        self.text_index
    }

    /// Close the open text block and return a `TextEnd` event.
    ///
    /// Returns `None` if no text block is open.
    pub fn close_text(&mut self) -> Option<AssistantMessageEvent> {
        self.text_index
            .take()
            .map(|idx| AssistantMessageEvent::TextEnd { content_index: idx })
    }

    /// Build a `TextDelta` event for the currently-open text block.
    ///
    /// Returns `None` if no text block is open (guards against stale state).
    pub fn text_delta(&self, delta: String) -> Option<AssistantMessageEvent> {
        self.text_index.map(|idx| AssistantMessageEvent::TextDelta {
            content_index: idx,
            delta,
        })
    }

    // ── Thinking block ─────────────────────────────────────────────────────

    /// Ensure a thinking block is open, allocating a new index if one is not
    /// already open.
    ///
    /// Returns a `ThinkingStart` event when a new block is opened, or `None`
    /// if the block was already open.
    pub fn ensure_thinking_open(&mut self) -> Option<AssistantMessageEvent> {
        if self.thinking_index.is_none() {
            let idx = self.alloc_index();
            self.thinking_index = Some((idx, None));
            Some(AssistantMessageEvent::ThinkingStart { content_index: idx })
        } else {
            None
        }
    }

    /// Return `true` if a thinking block is currently open.
    #[allow(dead_code)]
    #[inline]
    pub fn thinking_open(&self) -> bool {
        self.thinking_index.is_some()
    }

    /// Return the content index of the open thinking block, or `None` if no
    /// thinking block is open.
    #[allow(dead_code)]
    #[inline]
    pub fn thinking_index(&self) -> Option<usize> {
        self.thinking_index.as_ref().map(|(idx, _)| *idx)
    }

    /// Set the thinking signature, which will be included in the `ThinkingEnd`
    /// event when the block is closed.
    ///
    /// No-op if no thinking block is open.
    #[allow(dead_code)]
    pub fn set_thinking_signature(&mut self, signature: String) {
        if let Some((_, sig)) = &mut self.thinking_index {
            *sig = Some(signature);
        }
    }

    /// Close the open thinking block and return a `ThinkingEnd` event.
    ///
    /// If `signature` is `Some`, it overrides any signature previously set via
    /// [`set_thinking_signature`](Self::set_thinking_signature).
    ///
    /// Returns `None` if no thinking block is open.
    pub fn close_thinking(&mut self, signature: Option<String>) -> Option<AssistantMessageEvent> {
        self.thinking_index.take().map(|(idx, accumulated_sig)| {
            let final_sig = signature.or(accumulated_sig);
            AssistantMessageEvent::ThinkingEnd {
                content_index: idx,
                signature: final_sig,
            }
        })
    }

    /// Build a `ThinkingDelta` event for the currently-open thinking block.
    ///
    /// Returns `None` if no thinking block is open.
    #[allow(dead_code)]
    pub fn thinking_delta(&self, delta: String) -> Option<AssistantMessageEvent> {
        self.thinking_index
            .as_ref()
            .map(|(idx, _)| AssistantMessageEvent::ThinkingDelta {
                content_index: *idx,
                delta,
            })
    }

    // ── Tool-call blocks ────────────────────────────────────────────────────

    /// Open a new tool-call block with the given provider `id` and `name`.
    ///
    /// Allocates the next content index, registers the block as open, and
    /// returns a `(content_index, ToolCallStart)` pair. Multiple tool-call
    /// blocks may be open simultaneously.
    pub fn open_tool_call(&mut self, id: String, name: String) -> (usize, AssistantMessageEvent) {
        let idx = self.alloc_index();
        self.open_tool_calls.push(idx);
        let event = AssistantMessageEvent::ToolCallStart {
            content_index: idx,
            id,
            name,
        };
        (idx, event)
    }

    /// Close the tool-call block identified by `content_index`.
    ///
    /// Returns a `ToolCallEnd` event, or `None` if no open block with that
    /// index exists.
    #[allow(dead_code)]
    pub fn close_tool_call(&mut self, content_index: usize) -> Option<AssistantMessageEvent> {
        if let Some(pos) = self
            .open_tool_calls
            .iter()
            .position(|&ci| ci == content_index)
        {
            self.open_tool_calls.remove(pos);
            Some(AssistantMessageEvent::ToolCallEnd { content_index })
        } else {
            None
        }
    }

    /// Build a `ToolCallDelta` event without modifying block state.
    pub fn tool_call_delta(content_index: usize, delta: String) -> AssistantMessageEvent {
        AssistantMessageEvent::ToolCallDelta {
            content_index,
            delta,
        }
    }
}

// ─── StreamFinalize ────────────────────────────────────────────────────────

impl StreamFinalize for BlockAccumulator {
    fn drain_open_blocks(&mut self) -> Vec<OpenBlock> {
        // Collect all open blocks with their content indices so we can sort
        // them into close order (ascending content index).
        let mut entries: Vec<(usize, OpenBlock)> = Vec::new();

        if let Some((idx, sig)) = self.thinking_index.take() {
            entries.push((
                idx,
                OpenBlock::Thinking {
                    content_index: idx,
                    signature: sig,
                },
            ));
        }

        if let Some(idx) = self.text_index.take() {
            entries.push((idx, OpenBlock::Text { content_index: idx }));
        }

        for idx in self.open_tool_calls.drain(..) {
            entries.push((idx, OpenBlock::ToolCall { content_index: idx }));
        }

        entries.sort_unstable_by_key(|(idx, _)| *idx);
        entries.into_iter().map(|(_, block)| block).collect()
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
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
}
