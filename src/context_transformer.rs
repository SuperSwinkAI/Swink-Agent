//! Pluggable context transformation with compaction awareness.
//!
//! Replaces the bare `TransformContextFn` closure with a trait that supports
//! both transformation and compaction reporting.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::context::{CompactionReport, TokenCounter, compact_sliding_window_with};
use crate::types::AgentMessage;

pub trait ContextTransformer: Send + Sync {
    /// Transform the context messages in-place.
    ///
    /// Called synchronously before each LLM call. The `overflow` flag is true
    /// when the previous turn exceeded the context window.
    ///
    /// Returns `Some(CompactionReport)` if messages were dropped, `None` otherwise.
    fn transform(
        &self,
        messages: &mut Vec<AgentMessage>,
        overflow: bool,
    ) -> Option<CompactionReport>;

    /// Downcast support so the turn pipeline can inject the cached-prefix
    /// boundary into a built-in [`SlidingWindowTransformer`] each turn.
    ///
    /// The default impl returns a sentinel that downcasts to no concrete type
    /// the turn pipeline inspects, leaving custom transformers opaque and
    /// unaffected by the cache pipeline. Custom transformers that wish to
    /// participate in cache-protected compaction can override this to expose
    /// their concrete type.
    fn as_any(&self) -> &dyn std::any::Any {
        static SENTINEL: NoDowncast = NoDowncast;
        &SENTINEL
    }
}

/// Sentinel type returned by the default [`ContextTransformer::as_any`] impl.
///
/// Downcasts against this type always fail to match the concrete transformers
/// the turn pipeline cares about, which is the intended behavior for types
/// that don't opt in to the downcast hook.
struct NoDowncast;

/// Blanket impl for existing closures (backward compat).
impl<F: Fn(&mut Vec<AgentMessage>, bool) + Send + Sync> ContextTransformer for F {
    fn transform(
        &self,
        messages: &mut Vec<AgentMessage>,
        overflow: bool,
    ) -> Option<CompactionReport> {
        let before = messages.len();
        self(messages, overflow);
        let after = messages.len();
        if after < before {
            Some(CompactionReport {
                dropped_count: before - after,
                tokens_before: 0, // bare closures can't report token counts
                tokens_after: 0,
                overflow,
                dropped_messages: Vec::new(), // bare closures don't have access to the dropped slice
            })
        } else {
            None
        }
    }
}

/// Built-in sliding window context transformer with compaction reporting.
///
/// Wraps the same logic as [`sliding_window`](crate::sliding_window) but
/// captures compaction metrics for reporting.
///
/// Accepts an optional [`TokenCounter`] for pluggable token estimation.
/// When none is provided, the default `chars / 4` heuristic is used.
///
/// Supports runtime updates of `cached_prefix_len` via interior mutability so
/// the turn pipeline can propagate the cache boundary into each compaction pass
/// without needing `&mut` access to the transformer (which is shared behind
/// `Arc<dyn ContextTransformer>`).
pub struct SlidingWindowTransformer {
    normal_budget: usize,
    overflow_budget: usize,
    anchor: usize,
    token_counter: Option<Arc<dyn TokenCounter>>,
    /// Builder-set cached prefix length. When caching is active, protects
    /// this many leading messages from compaction unless `published_prefix`
    /// overrides it.
    cached_prefix_len: usize,
    /// Runtime-published cached prefix length, set by the turn pipeline
    /// through interior mutability. Zero means "no runtime publish yet";
    /// otherwise it takes precedence over `cached_prefix_len`.
    published_prefix: AtomicUsize,
}

impl SlidingWindowTransformer {
    /// Create a new sliding window transformer.
    ///
    /// # Arguments
    ///
    /// * `normal_budget` - Token budget under normal operation.
    /// * `overflow_budget` - Smaller token budget used when overflow is signaled.
    /// * `anchor` - Number of messages at the start to always preserve.
    #[must_use]
    pub const fn new(normal_budget: usize, overflow_budget: usize, anchor: usize) -> Self {
        Self {
            normal_budget,
            overflow_budget,
            anchor,
            token_counter: None,
            cached_prefix_len: 0,
            published_prefix: AtomicUsize::new(0),
        }
    }

    #[must_use]
    pub fn with_token_counter(mut self, counter: Arc<dyn TokenCounter>) -> Self {
        self.token_counter = Some(counter);
        self
    }

    /// Set the cached prefix length to protect from compaction.
    ///
    /// When caching is active, the effective anchor is `max(anchor, cached_prefix_len)`.
    #[must_use]
    pub const fn with_cached_prefix_len(mut self, len: usize) -> Self {
        self.cached_prefix_len = len;
        self
    }

    /// Set the cached prefix length on a uniquely-owned transformer.
    ///
    /// Use [`Self::publish_cached_prefix`] when the transformer is shared
    /// behind an `Arc` at runtime.
    pub const fn set_cached_prefix_len(&mut self, len: usize) {
        self.cached_prefix_len = len;
    }

    /// Publish a new cached prefix length through interior mutability.
    ///
    /// Used by the turn pipeline to update the boundary before each
    /// compaction pass without taking `&mut` on the shared `Arc<dyn ...>`.
    /// A non-zero value takes precedence over the builder-set field.
    pub fn publish_cached_prefix(&self, len: usize) {
        self.published_prefix.store(len, Ordering::Relaxed);
    }

    /// Read the current effective cached prefix length — the runtime
    /// publish if set, otherwise the builder-set value.
    #[must_use]
    pub fn cached_prefix_len(&self) -> usize {
        let published = self.published_prefix.load(Ordering::Relaxed);
        if published > 0 {
            published
        } else {
            self.cached_prefix_len
        }
    }
}

impl ContextTransformer for SlidingWindowTransformer {
    fn transform(
        &self,
        messages: &mut Vec<AgentMessage>,
        overflow: bool,
    ) -> Option<CompactionReport> {
        let budget = if overflow {
            self.overflow_budget
        } else {
            self.normal_budget
        };

        let cached_prefix = self.cached_prefix_len();
        let effective_anchor = self.anchor.max(cached_prefix);
        let counter_ref = self.token_counter.as_deref();
        let mut report =
            compact_sliding_window_with(messages, budget, effective_anchor, counter_ref)?;
        report.overflow = overflow;
        Some(report)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
#[path = "context_transformer_tests.rs"]
mod tests;
