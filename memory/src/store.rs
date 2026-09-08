//! Session storage trait for pluggable persistence backends.

use std::io;
use std::sync::Once;

use swink_agent::{AgentMessage, CustomMessageRegistry};

use crate::entry::SessionEntry;
use crate::interrupt::InterruptState;
use crate::load_options::LoadOptions;
use crate::meta::SessionMeta;
use crate::search::{SessionHit, SessionSearchOptions};

/// Pluggable session persistence.
///
/// All save/load methods use [`AgentMessage`] as the canonical message type,
/// preserving both LLM and custom messages without silent data loss.
///
/// The default implementation ([`JsonlSessionStore`](crate::JsonlSessionStore))
/// uses JSONL files, but alternative backends (`SQLite`, S3, etc.) can
/// implement this trait directly.
pub trait SessionStore: Send + Sync {
    /// Persist a session, including both LLM and custom messages.
    ///
    /// Overwrites any existing session with the same ID. Custom messages that
    /// cannot be serialized are logged and skipped — callers should ensure
    /// custom types implement `CustomMessage::to_json` and
    /// `CustomMessage::type_name`.
    fn save(&self, id: &str, meta: &SessionMeta, messages: &[AgentMessage]) -> io::Result<()>;

    /// Persist a session transcript plus its state snapshot.
    ///
    /// Stores with optimistic-concurrency metadata should return the metadata
    /// as persisted on disk so callers can keep their local sequence in sync.
    /// Backends must implement this explicitly if they support atomic
    /// transcript+state persistence; the default returns
    /// [`io::ErrorKind::Unsupported`] so callers do not accidentally rely on a
    /// non-atomic fallback.
    fn save_full(
        &self,
        id: &str,
        meta: &SessionMeta,
        messages: &[AgentMessage],
        state: &serde_json::Value,
    ) -> io::Result<SessionMeta> {
        let _ = (id, meta, messages, state);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "SessionStore::save_full requires an explicit atomic backend implementation",
        ))
    }

    /// Append messages to an existing session without rewriting the entire file.
    fn append(&self, id: &str, messages: &[AgentMessage]) -> io::Result<()>;

    /// Load a session by ID.
    ///
    /// If `registry` is `Some`, custom messages are deserialized using the
    /// provided registry. If `None`, custom messages are skipped.
    fn load(
        &self,
        id: &str,
        registry: Option<&CustomMessageRegistry>,
    ) -> io::Result<(SessionMeta, Vec<AgentMessage>)>;

    /// Load a session transcript plus its state snapshot from one consistent
    /// read boundary.
    ///
    /// Backends must implement this explicitly if they support atomic
    /// transcript+state restore; the default returns
    /// [`io::ErrorKind::Unsupported`] so callers do not silently mix
    /// transcript and state revisions via separate reads.
    fn load_full(
        &self,
        id: &str,
        registry: Option<&CustomMessageRegistry>,
    ) -> io::Result<(SessionMeta, Vec<AgentMessage>, Option<serde_json::Value>)> {
        let _ = (id, registry);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "SessionStore::load_full requires an explicit atomic backend implementation",
        ))
    }

    /// List all saved sessions, sorted by last updated (newest first).
    fn list(&self) -> io::Result<Vec<SessionMeta>>;

    /// Delete a session by ID.
    fn delete(&self, id: &str) -> io::Result<()>;

    /// Save session state snapshot.
    ///
    /// The default implementation is a no-op that **discards the state**:
    /// the snapshot is dropped and a later [`load_state`](Self::load_state)
    /// returns `None`. It exists only for backward compatibility with
    /// pre-034 store implementations (spec 034 FR-018). The first time the
    /// default runs in a process it emits a `tracing::warn!` so the data
    /// loss is visible; subsequent calls are silent.
    ///
    /// **Planned breaking change:** `save_state` will become a required
    /// (non-defaulted) trait method in the next major version bump. Custom
    /// `SessionStore` implementations should override it (together with
    /// [`load_state`](Self::load_state)) now.
    fn save_state(&self, id: &str, state: &serde_json::Value) -> io::Result<()> {
        static WARN_ONCE: Once = Once::new();
        WARN_ONCE.call_once(|| {
            tracing::warn!(
                session_id = %id,
                "SessionStore::save_state default no-op invoked: this store does not \
                 implement state persistence, so session state will be lost. Override \
                 save_state and load_state; they become required methods in the next \
                 major version. This warning is emitted once per process."
            );
        });
        let _ = state;
        Ok(())
    }

    /// Load session state snapshot.
    ///
    /// The default implementation always returns `Ok(None)` (empty state),
    /// even if a matching [`save_state`](Self::save_state) call appeared to
    /// succeed. It exists only for backward compatibility with pre-034 store
    /// implementations (spec 034 FR-018). The first time the default runs in
    /// a process it emits a `tracing::warn!` so the silent fallback is
    /// visible; subsequent calls are silent.
    ///
    /// **Planned breaking change:** `load_state` will become a required
    /// (non-defaulted) trait method in the next major version bump. Custom
    /// `SessionStore` implementations should override it (together with
    /// [`save_state`](Self::save_state)) now.
    fn load_state(&self, id: &str) -> io::Result<Option<serde_json::Value>> {
        static WARN_ONCE: Once = Once::new();
        WARN_ONCE.call_once(|| {
            tracing::warn!(
                session_id = %id,
                "SessionStore::load_state default no-op invoked: this store does not \
                 implement state persistence, so loads always return empty state. \
                 Override save_state and load_state; they become required methods in \
                 the next major version. This warning is emitted once per process."
            );
        });
        Ok(None)
    }

    /// Persist interrupt state for a session.
    ///
    /// Stores the interrupt as `{session_id}.interrupt.json`. Overwrites any
    /// existing interrupt for the same session. Default: no-op.
    fn save_interrupt(&self, id: &str, state: &InterruptState) -> io::Result<()> {
        let _ = (id, state);
        Ok(())
    }

    /// Load interrupt state for a session.
    ///
    /// Returns `Some` if an interrupt file exists, `None` otherwise.
    /// Returns an error if the file exists but is corrupted. Default: `None`.
    fn load_interrupt(&self, id: &str) -> io::Result<Option<InterruptState>> {
        let _ = id;
        Ok(None)
    }

    /// Clear interrupt state for a session.
    ///
    /// Deletes the `{session_id}.interrupt.json` file. Idempotent — safe to
    /// call if no interrupt exists. Default: no-op.
    fn clear_interrupt(&self, id: &str) -> io::Result<()> {
        let _ = id;
        Ok(())
    }

    /// Load a session with filtering options.
    ///
    /// Returns metadata and a filtered subset of session entries based on the
    /// provided [`LoadOptions`]. Default options return the full session.
    fn load_with_options(
        &self,
        id: &str,
        options: &LoadOptions,
    ) -> io::Result<(SessionMeta, Vec<SessionEntry>)>;

    /// Search across persisted sessions.
    ///
    /// Backends that do not support search can rely on the default empty
    /// result for backward compatibility.
    fn search(&self, query: &str, options: &SessionSearchOptions) -> io::Result<Vec<SessionHit>> {
        let _ = (query, options);
        Ok(Vec::new())
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
