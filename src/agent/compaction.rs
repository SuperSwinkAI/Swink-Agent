//! Manual (host-triggered) context compaction.
//!
//! Automatic compaction runs inside the turn loop before each LLM call. This
//! module adds an on-demand entry point so hosts (e.g. a TUI `/compact`
//! command) can trigger the same compaction pipeline between turns and render
//! the resulting [`CompactionReport`] synchronously.

use std::sync::Arc;

use crate::context::CompactionReport;
use crate::error::AgentError;
use crate::loop_::AgentEvent;

use super::Agent;

impl Agent {
    /// Run the configured context transformer(s) against the stored history
    /// now, as if a context overflow had occurred.
    ///
    /// This is the manual counterpart to the automatic compaction that runs
    /// inside the turn loop. Use it to implement host-level compact-on-demand
    /// commands (e.g. a TUI `/compact`) between turns: the pruned history is
    /// persisted on the agent, so the next request reflects it.
    ///
    /// # Semantics
    ///
    /// Manual compaction mirrors the loop's transformer pipeline exactly:
    ///
    /// - Transformers are invoked with `overflow = true` — a host asking for
    ///   compaction wants maximal pruning, so the overflow budget applies
    ///   (for [`SlidingWindowTransformer`](crate::SlidingWindowTransformer),
    ///   the `overflow_budget` rather than the `normal_budget`).
    /// - When both an async and a sync transformer are configured, the async
    ///   transformer runs first and the sync transformer second (the loop's
    ///   order). One [`AgentEvent::ContextCompacted`] is emitted per
    ///   transformer that reports compaction, and the *last* report is
    ///   returned.
    /// - Each `ContextCompacted` event is dispatched through the normal event
    ///   path (subscribers and event forwarders), so existing host event
    ///   handling renders manual compaction identically to loop-emitted
    ///   compaction.
    /// - A transformer that declines to compact (e.g. the sliding window when
    ///   the history is already under budget) returns `None` and emits no
    ///   event — the same no-op behavior as in the loop.
    ///
    /// # Returns
    ///
    /// - `Ok(Some(report))` — at least one transformer pruned the history;
    ///   `report` is the last transformer's [`CompactionReport`].
    /// - `Ok(None)` — no transformer is configured, or every configured
    ///   transformer declined (history under budget). The history is
    ///   untouched and no event is emitted.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::AlreadyRunning`] if an agent loop is currently
    /// active — compacting while a turn is in flight would race the loop's
    /// view of the conversation history.
    pub async fn compact_context(&mut self) -> Result<Option<CompactionReport>, AgentError> {
        self.check_not_running()?;

        // Async transformer runs first (mirrors `run_context_transformers`).
        let async_report = match self.async_transform_context.as_ref().map(Arc::clone) {
            Some(transformer) => transformer.transform(&mut self.state.messages, true).await,
            None => None,
        };
        if let Some(ref report) = async_report {
            self.dispatch_event(&AgentEvent::ContextCompacted {
                report: report.clone(),
            });
        }

        // Sync transformer runs second.
        let sync_report = match self.transform_context.as_ref().map(Arc::clone) {
            Some(transformer) => transformer.transform(&mut self.state.messages, true),
            None => None,
        };
        if let Some(ref report) = sync_report {
            self.dispatch_event(&AgentEvent::ContextCompacted {
                report: report.clone(),
            });
        }

        Ok(sync_report.or(async_report))
    }
}
