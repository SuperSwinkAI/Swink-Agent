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

/// Metadata for a stored context version (returned by `list_versions`).
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

fn extract_llm_messages(messages: &[AgentMessage]) -> Vec<LlmMessage> {
    messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Llm(llm) => Some(llm.clone()),
            AgentMessage::Custom(_) => None,
        })
        .collect()
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
        let before_len = messages.len();

        // Snapshot LLM messages before compaction so we can identify dropped ones.
        let snapshot: Vec<LlmMessage> = extract_llm_messages(messages);

        // Run the inner transformer.
        let report = self.inner.transform(messages, overflow)?;

        let after_len = messages.len();
        let dropped_count = before_len - after_len;

        if dropped_count == 0 {
            return Some(report);
        }

        // Collect the LLM messages that survived compaction.
        let surviving: Vec<LlmMessage> = extract_llm_messages(messages);

        // The dropped LLM messages are those in the snapshot but not in the
        // surviving set. Sliding window removes a contiguous middle section,
        // so we can diff by finding the anchor prefix and tail suffix that
        // match, and the middle is what was dropped.
        let anchor_count = snapshot
            .iter()
            .zip(surviving.iter())
            .take_while(|(a, b)| format!("{a:?}") == format!("{b:?}"))
            .count();

        let tail_count = surviving.len().saturating_sub(anchor_count);
        let snapshot_tail_start = snapshot.len().saturating_sub(tail_count);

        let dropped_messages: Vec<LlmMessage> = if anchor_count < snapshot_tail_start {
            snapshot[anchor_count..snapshot_tail_start].to_vec()
        } else {
            Vec::new()
        };

        if dropped_messages.is_empty() {
            return Some(report);
        }

        // Build the version.
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.turn_counter += 1;

        let summary = self
            .summarizer
            .as_ref()
            .and_then(|s| s.summarize(&dropped_messages));

        let version = ContextVersion {
            version: state.next_version,
            turn: state.turn_counter,
            timestamp: crate::util::now_timestamp(),
            messages: dropped_messages,
            summary,
        };

        state.next_version += 1;
        drop(state);

        self.store.save_version(&version);

        Some(report)
    }
}

#[cfg(test)]
mod tests {
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
}
