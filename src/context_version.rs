//! Context versioning and multi-layer memory.
//!
//! Provides snapshot-based context versioning with optional pre-computed
//! summarization. When context is compacted, dropped messages are captured
//! as a [`ContextVersion`] and stored via a pluggable [`ContextVersionStore`].
//! A [`ContextSummarizer`] can produce summaries that accompany each version,
//! enabling RAG and hierarchical context patterns.

use std::sync::{Arc, Mutex};

use crate::context::CompactionReport;
use crate::context_transformer::ContextTransformer;
use crate::types::{AgentMessage, LlmMessage};

// ─── ContextVersion ──────────────────────────────────────────────────────────

/// A snapshot of messages captured at a point in time.
///
/// Created during compaction when messages are dropped from the active context.
/// Each version records the version number, turn number, timestamp, the dropped
/// LLM messages, and an optional summary.
///
/// Only `LlmMessage` variants are stored; `CustomMessage` values are filtered
/// out since they are not cloneable.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ContextVersion {
    /// Monotonically increasing version number (starts at 1).
    pub version: u64,
    /// Turn number when this version was created.
    pub turn: u64,
    /// Unix timestamp (seconds) when this version was created.
    pub timestamp: u64,
    /// The LLM messages that were dropped during compaction.
    pub messages: Vec<LlmMessage>,
    /// Optional pre-computed summary of the dropped messages.
    pub summary: Option<String>,
}

impl ContextVersion {
    /// Create a new context version snapshot with no summary.
    #[must_use]
    pub const fn new(version: u64, turn: u64, timestamp: u64, messages: Vec<LlmMessage>) -> Self {
        Self {
            version,
            turn,
            timestamp,
            messages,
            summary: None,
        }
    }

    /// Attach a pre-computed summary of the dropped messages.
    #[must_use]
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }
}

/// Metadata for a stored context version (returned by `list_versions`).
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ContextVersionMeta {
    /// Version number.
    pub version: u64,
    /// Turn number when created.
    pub turn: u64,
    /// Unix timestamp when created.
    pub timestamp: u64,
    /// Number of messages in this version.
    pub message_count: usize,
    /// Whether a summary is available.
    pub has_summary: bool,
}

impl ContextVersionMeta {
    /// Create a new context version metadata record.
    #[must_use]
    pub const fn new(
        version: u64,
        turn: u64,
        timestamp: u64,
        message_count: usize,
        has_summary: bool,
    ) -> Self {
        Self {
            version,
            turn,
            timestamp,
            message_count,
            has_summary,
        }
    }
}

// ─── ContextVersionStore ─────────────────────────────────────────────────────

/// Pluggable storage for context version snapshots.
///
/// Implementations persist dropped messages from compaction for later retrieval,
/// enabling RAG-style recall of earlier conversation context.
pub trait ContextVersionStore: Send + Sync {
    /// Save a context version. Called automatically during compaction.
    fn save_version(&self, version: &ContextVersion);

    /// Load a specific version by number.
    fn load_version(&self, version: u64) -> Option<ContextVersion>;

    /// List metadata for all stored versions, ordered by version number.
    fn list_versions(&self) -> Vec<ContextVersionMeta>;

    /// Load the most recent version, if any.
    fn latest_version(&self) -> Option<ContextVersion> {
        let versions = self.list_versions();
        versions
            .last()
            .and_then(|meta| self.load_version(meta.version))
    }
}

// ─── ContextSummarizer ───────────────────────────────────────────────────────

/// Pre-computed summarization of dropped context messages.
///
/// Called synchronously during compaction to produce a summary of the messages
/// being evicted. The summary is stored alongside the version and can be
/// injected back into context (e.g., via `SummarizingCompactor` in the memory
/// crate).
///
/// For async summarization (e.g., LLM calls), pre-compute the summary
/// externally and attach it via the version store.
pub trait ContextSummarizer: Send + Sync {
    /// Produce a summary of the given messages.
    ///
    /// Called with the messages that are about to be dropped during compaction.
    /// Returns `None` if summarization is not possible or not desired.
    fn summarize(&self, messages: &[LlmMessage]) -> Option<String>;
}

// ─── InMemoryVersionStore ────────────────────────────────────────────────────

/// In-memory implementation of [`ContextVersionStore`].
///
/// Suitable for single-session usage and testing. Versions are stored in a
/// `Vec` behind a `Mutex`.
pub struct InMemoryVersionStore {
    versions: Mutex<Vec<ContextVersion>>,
}

impl InMemoryVersionStore {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            versions: Mutex::new(Vec::new()),
        }
    }

    pub fn len(&self) -> usize {
        self.versions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for InMemoryVersionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextVersionStore for InMemoryVersionStore {
    fn save_version(&self, version: &ContextVersion) {
        let mut guard = self
            .versions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.push(version.clone());
    }

    fn load_version(&self, version: u64) -> Option<ContextVersion> {
        let guard = self
            .versions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.iter().find(|v| v.version == version).cloned()
    }

    fn list_versions(&self) -> Vec<ContextVersionMeta> {
        let guard = self
            .versions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard
            .iter()
            .map(|v| ContextVersionMeta {
                version: v.version,
                turn: v.turn,
                timestamp: v.timestamp,
                message_count: v.messages.len(),
                has_summary: v.summary.is_some(),
            })
            .collect()
    }
}

// ─── VersioningTransformer ───────────────────────────────────────────────────

/// A context transformer that captures dropped messages as versioned snapshots.
///
/// Wraps an inner [`ContextTransformer`] (typically a sliding window) and
/// stores evicted messages via a [`ContextVersionStore`]. An optional
/// [`ContextSummarizer`] produces summaries for each version.
///
/// # Example
///
/// ```rust,ignore
/// use swink_agent::{
///     SlidingWindowTransformer, VersioningTransformer,
///     InMemoryVersionStore,
/// };
/// use std::sync::Arc;
///
/// let store = Arc::new(InMemoryVersionStore::new());
/// let inner = SlidingWindowTransformer::new(100_000, 50_000, 2);
/// let transformer = VersioningTransformer::new(inner, store);
///
/// let agent = AgentOptions::new(/* ... */)
///     .with_transform_context(transformer);
/// ```
pub struct VersioningTransformer {
    inner: Box<dyn ContextTransformer>,
    store: Arc<dyn ContextVersionStore>,
    summarizer: Option<Arc<dyn ContextSummarizer>>,
    state: Mutex<VersioningState>,
}

struct VersioningState {
    next_version: u64,
    turn_counter: u64,
}

impl VersioningTransformer {
    /// Create a new versioning transformer wrapping an inner transformer.
    pub fn new(
        inner: impl ContextTransformer + 'static,
        store: Arc<dyn ContextVersionStore>,
    ) -> Self {
        Self {
            inner: Box::new(inner),
            store,
            summarizer: None,
            state: Mutex::new(VersioningState {
                next_version: 1,
                turn_counter: 0,
            }),
        }
    }

    /// Attach a summarizer that produces summaries for each version.
    #[must_use]
    pub fn with_summarizer(mut self, summarizer: Arc<dyn ContextSummarizer>) -> Self {
        self.summarizer = Some(summarizer);
        self
    }

    /// Access the underlying version store.
    pub fn store(&self) -> &Arc<dyn ContextVersionStore> {
        &self.store
    }
}

impl ContextTransformer for VersioningTransformer {
    fn transform(
        &self,
        messages: &mut Vec<AgentMessage>,
        overflow: bool,
    ) -> Option<CompactionReport> {
        // Run the inner transformer. The report carries the dropped LLM messages
        // directly — no snapshot/diff reconstruction needed.
        let report = self.inner.transform(messages, overflow)?;

        if report.dropped_messages.is_empty() {
            return Some(report);
        }

        // Build the version from the report's explicit dropped-message list.
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.turn_counter += 1;

        let summary = self
            .summarizer
            .as_ref()
            .and_then(|s| s.summarize(&report.dropped_messages));

        let version = ContextVersion {
            version: state.next_version,
            turn: state.turn_counter,
            timestamp: crate::util::now_timestamp(),
            messages: report.dropped_messages.clone(),
            summary,
        };

        state.next_version += 1;
        drop(state);

        self.store.save_version(&version);

        Some(report)
    }
}

#[cfg(test)]
#[path = "context_version_tests.rs"]
mod tests;
