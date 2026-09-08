//! Context compaction utilities for managing conversation history size.

use serde::{Deserialize, Serialize};

use crate::types::{AgentMessage, ContentBlock, LlmMessage};

// ─── Token Counter Trait ────────────────────────────────────────────────────

/// Pluggable token counting strategy.
///
/// Implement this trait to replace the built-in `chars / 4` heuristic with
/// tiktoken, a provider-native tokenizer, or any other counting scheme.
pub trait TokenCounter: Send + Sync {
    /// Return the estimated token count for a single message.
    fn count_tokens(&self, message: &AgentMessage) -> usize;
}

/// Default token counter using the `chars / 4` heuristic.
///
/// `LlmMessage` variants: sums character lengths of all text-bearing content
/// blocks and divides by 4. `CustomMessage` variants count as 100 tokens flat.
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub struct DefaultTokenCounter;

impl DefaultTokenCounter {
    /// Create a new default token counter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for DefaultTokenCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenCounter for DefaultTokenCounter {
    fn count_tokens(&self, message: &AgentMessage) -> usize {
        match message {
            AgentMessage::Llm(llm) => {
                let chars: usize = content_blocks(llm)
                    .iter()
                    .map(|b| match b {
                        ContentBlock::Text { text } => text.len(),
                        ContentBlock::Thinking { thinking, .. } => thinking.len(),
                        ContentBlock::ToolCall { arguments, .. } => arguments.to_string().len(),
                        ContentBlock::Image { .. } => 0,
                        ContentBlock::Extension { data, .. } => data.to_string().len(),
                    })
                    .sum();
                chars / 4
            }
            AgentMessage::Custom(_) => 100,
        }
    }
}

/// Estimate token count using `chars / 4` for LLM messages, 100 flat for custom.
///
/// For pluggable counting, use a [`TokenCounter`] implementation instead.
pub fn estimate_tokens(msg: &AgentMessage) -> usize {
    DefaultTokenCounter.count_tokens(msg)
}

/// Estimate the token cost of a request's tool schemas.
///
/// Prices each tool's name + description + serialized parameters schema at
/// `chars / 4`, separately from the message history, so hosts budgeting
/// injected context (repo maps, file mentions) against a model's context
/// window can subtract the fixed overhead first.
pub fn estimate_tool_schema_tokens(tools: &[std::sync::Arc<dyn crate::tool::AgentTool>]) -> usize {
    tools
        .iter()
        .map(|tool| {
            (tool.name().len()
                + tool.description().len()
                + tool.parameters_schema().to_string().len())
                / 4
        })
        .sum()
}

/// Estimate the token cost of an entire request context.
///
/// Sums the system prompt, tool schemas, and message history with the same
/// `chars / 4` heuristic the loop's built-in [`DefaultTokenCounter`] uses
/// for messages.
///
/// Use this instead of re-deriving the arithmetic downstream: a host budget
/// computed here stays in agreement with the loop's own estimates if the
/// heuristic ever changes.
pub fn estimate_context_tokens(context: &crate::types::AgentContext) -> usize {
    context.system_prompt.len() / 4
        + estimate_tool_schema_tokens(&context.tools)
        + context.messages.iter().map(estimate_tokens).sum::<usize>()
}

fn content_blocks(msg: &LlmMessage) -> &[ContentBlock] {
    match msg {
        LlmMessage::User(m) => &m.content,
        LlmMessage::Assistant(m) => &m.content,
        LlmMessage::ToolResult(m) => &m.content,
    }
}

fn is_tool_result(messages: &[AgentMessage], idx: usize) -> bool {
    matches!(
        messages.get(idx),
        Some(AgentMessage::Llm(LlmMessage::ToolResult(_)))
    )
}

fn tool_call_ids(message: &AgentMessage) -> Option<Vec<&str>> {
    match message {
        AgentMessage::Llm(LlmMessage::Assistant(assistant)) => {
            let ids: Vec<&str> = assistant
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolCall { id, .. } => Some(id.as_str()),
                    _ => None,
                })
                .collect();
            (!ids.is_empty()).then_some(ids)
        }
        _ => None,
    }
}

fn tool_result_id(message: &AgentMessage) -> Option<&str> {
    match message {
        AgentMessage::Llm(LlmMessage::ToolResult(result)) => Some(result.tool_call_id.as_str()),
        _ => None,
    }
}

fn extend_anchor_for_tool_results(messages: &[AgentMessage], anchor_end: usize) -> usize {
    if anchor_end == 0 || anchor_end >= messages.len() {
        return anchor_end;
    }

    let mut assistant_idx = anchor_end - 1;
    while is_tool_result(messages, assistant_idx) {
        if assistant_idx == 0 {
            return anchor_end;
        }
        assistant_idx -= 1;
    }

    let Some(call_ids) = tool_call_ids(&messages[assistant_idx]) else {
        return anchor_end;
    };

    let mut group_end = assistant_idx + 1;
    while group_end < messages.len() {
        let Some(result_id) = tool_result_id(&messages[group_end]) else {
            break;
        };
        if !call_ids.contains(&result_id) {
            break;
        }
        group_end += 1;
    }

    anchor_end.max(group_end)
}

/// Result of a context transformation pass.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionReport {
    /// Number of messages that were removed during compaction.
    pub dropped_count: usize,
    /// Estimated tokens before compaction.
    pub tokens_before: usize,
    /// Estimated tokens after compaction.
    pub tokens_after: usize,
    /// Whether compaction was triggered by overflow.
    pub overflow: bool,
    /// The LLM messages that were dropped during this compaction pass.
    ///
    /// Only `LlmMessage` variants are included; `CustomMessage` values are
    /// filtered out. Populated by the sliding-window compaction routine; empty for
    /// bare-closure transformers that don't have access to the dropped slice.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped_messages: Vec<LlmMessage>,
}

impl CompactionReport {
    /// Create a new compaction report with no dropped messages recorded.
    #[must_use]
    pub const fn new(
        dropped_count: usize,
        tokens_before: usize,
        tokens_after: usize,
        overflow: bool,
    ) -> Self {
        Self {
            dropped_count,
            tokens_before,
            tokens_after,
            overflow,
            dropped_messages: Vec::new(),
        }
    }

    /// Attach the LLM messages that were dropped during this compaction pass.
    #[must_use]
    pub fn with_dropped_messages(mut self, dropped_messages: Vec<LlmMessage>) -> Self {
        self.dropped_messages = dropped_messages;
        self
    }
}

/// Core sliding window compaction algorithm.
///
/// Keeps messages within `budget` by removing older messages from the middle
/// while preserving the first `anchor` messages and as many recent messages as
/// fit. Tool-call / tool-result pairs are kept together even if this exceeds
/// the budget.
///
/// When `counter` is `None` the [`DefaultTokenCounter`] heuristic is used.
///
/// Returns `Some(CompactionReport)` when messages were dropped, `None` otherwise.
pub fn compact_sliding_window(
    messages: &mut Vec<AgentMessage>,
    budget: usize,
    anchor: usize,
) -> Option<CompactionReport> {
    compact_sliding_window_with(messages, budget, anchor, None)
}

/// Like [`compact_sliding_window`] but accepts a pluggable [`TokenCounter`].
pub fn compact_sliding_window_with(
    messages: &mut Vec<AgentMessage>,
    budget: usize,
    anchor: usize,
    counter: Option<&dyn TokenCounter>,
) -> Option<CompactionReport> {
    let default = DefaultTokenCounter;
    let counter: &dyn TokenCounter = counter.unwrap_or(&default);

    let count = |m: &AgentMessage| counter.count_tokens(m);

    let tokens_before: usize = messages.iter().map(count).sum();
    if tokens_before <= budget {
        return None;
    }

    let len = messages.len();
    let effective_anchor = extend_anchor_for_tool_results(messages, anchor.min(len));

    // Calculate tokens used by anchor messages.
    let anchor_tokens: usize = messages[..effective_anchor].iter().map(count).sum();

    let remaining_budget = budget.saturating_sub(anchor_tokens);

    // Walk backwards from the end, accumulating messages that fit.
    let mut tail_tokens = 0;
    let mut tail_start = len;

    for i in (effective_anchor..len).rev() {
        let msg_tokens = count(&messages[i]);
        if tail_tokens + msg_tokens > remaining_budget {
            break;
        }
        tail_tokens += msg_tokens;
        tail_start = i;
    }

    // Adjust tail_start backward to avoid splitting tool-call / tool-result
    // pairs. If tail_start lands on a tool-result, include the preceding
    // assistant message too (correctness > token count).
    while tail_start > effective_anchor && tail_start < len && is_tool_result(messages, tail_start)
    {
        tail_start -= 1;
    }

    // If nothing would be removed, bail out.
    if tail_start <= effective_anchor {
        return None;
    }

    let dropped_count = tail_start - effective_anchor;

    // Collect the dropped LLM messages before modifying the slice.
    let dropped_messages: Vec<LlmMessage> = messages[effective_anchor..tail_start]
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Llm(llm) => Some(llm.clone()),
            AgentMessage::Custom(_) => None,
        })
        .collect();

    // Build the compacted list: anchor messages + tail messages.
    let tail: Vec<AgentMessage> = messages.drain(tail_start..).collect();
    messages.truncate(effective_anchor);
    messages.extend(tail);

    let tokens_after: usize = messages.iter().map(count).sum();

    Some(CompactionReport {
        dropped_count,
        tokens_before,
        tokens_after,
        overflow: false,
        dropped_messages,
    })
}

/// Create a sliding-window context compaction function.
///
/// Keeps messages within an estimated token budget by removing older messages
/// from the middle while preserving the first `anchor` messages and as many
/// recent messages as fit.
///
/// When `overflow` is true (context window exceeded), uses `overflow_budget`
/// instead of `normal_budget`.
#[deprecated(since = "0.5.0", note = "Use SlidingWindowTransformer instead")]
pub fn sliding_window(
    normal_budget: usize,
    overflow_budget: usize,
    anchor: usize,
) -> impl Fn(&mut Vec<AgentMessage>, bool) + Send + Sync {
    move |messages: &mut Vec<AgentMessage>, overflow: bool| {
        let budget = if overflow {
            overflow_budget
        } else {
            normal_budget
        };
        compact_sliding_window(messages, budget, anchor);
    }
}

/// Estimate whether the context exceeds the model's maximum context window.
///
/// Returns `true` if the estimated token count exceeds
/// `model.capabilities.max_context_window`. Returns `false` if the model has
/// no known context window limit.
pub fn is_context_overflow(
    messages: &[AgentMessage],
    model: &crate::types::ModelSpec,
    counter: Option<&dyn TokenCounter>,
) -> bool {
    let max_window = model
        .capabilities
        .as_ref()
        .and_then(|c| c.max_context_window);

    let Some(max_window) = max_window else {
        return false;
    };

    let default = DefaultTokenCounter;
    let counter: &dyn TokenCounter = counter.unwrap_or(&default);

    let total_tokens: usize = messages.iter().map(|m| counter.count_tokens(m)).sum();
    total_tokens as u64 > max_window
}

#[cfg(test)]
#[path = "context_tests.rs"]
mod tests;
