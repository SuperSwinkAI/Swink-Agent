//! Session storage trait for pluggable persistence backends.

use std::io;

use swink_agent::{AgentMessage, CustomMessageRegistry};

use crate::entry::SessionEntry;
use crate::interrupt::InterruptState;
use crate::load_options::LoadOptions;
use crate::meta::SessionMeta;

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

    /// List all saved sessions, sorted by last updated (newest first).
    fn list(&self) -> io::Result<Vec<SessionMeta>>;

    /// Delete a session by ID.
    fn delete(&self, id: &str) -> io::Result<()>;

    /// Save session state snapshot. Default: no-op.
    fn save_state(&self, id: &str, state: &serde_json::Value) -> io::Result<()> {
        let _ = (id, state);
        Ok(())
    }

    /// Load session state snapshot. Default: `None` (empty state).
    fn load_state(&self, id: &str) -> io::Result<Option<serde_json::Value>> {
        let _ = id;
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
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use chrono::Utc;
    use serde_json::json;

    use super::*;

    struct CountingStore {
        save_calls: Arc<AtomicUsize>,
        save_state_calls: Arc<AtomicUsize>,
    }

    impl SessionStore for CountingStore {
        fn save(
            &self,
            _id: &str,
            _meta: &SessionMeta,
            _messages: &[AgentMessage],
        ) -> io::Result<()> {
            self.save_calls.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn append(&self, _id: &str, _messages: &[AgentMessage]) -> io::Result<()> {
            Ok(())
        }

        fn load(
            &self,
            _id: &str,
            _registry: Option<&CustomMessageRegistry>,
        ) -> io::Result<(SessionMeta, Vec<AgentMessage>)> {
            Ok((sample_meta(), Vec::new()))
        }

        fn list(&self) -> io::Result<Vec<SessionMeta>> {
            Ok(Vec::new())
        }

        fn delete(&self, _id: &str) -> io::Result<()> {
            Ok(())
        }

        fn save_state(&self, _id: &str, _state: &serde_json::Value) -> io::Result<()> {
            self.save_state_calls.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn load_with_options(
            &self,
            _id: &str,
            _options: &LoadOptions,
        ) -> io::Result<(SessionMeta, Vec<SessionEntry>)> {
            Ok((sample_meta(), Vec::new()))
        }
    }

    fn sample_meta() -> SessionMeta {
        SessionMeta {
            id: "session-1".to_string(),
            title: "Session 1".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            version: 1,
            sequence: 0,
        }
    }

    #[test]
    fn default_save_full_rejects_non_atomic_fallback_without_writing() {
        let save_calls = Arc::new(AtomicUsize::new(0));
        let save_state_calls = Arc::new(AtomicUsize::new(0));
        let store = CountingStore {
            save_calls: Arc::clone(&save_calls),
            save_state_calls: Arc::clone(&save_state_calls),
        };

        let error = store
            .save_full("session-1", &sample_meta(), &[], &json!({"draft": true}))
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert_eq!(save_calls.load(Ordering::Relaxed), 0);
        assert_eq!(save_state_calls.load(Ordering::Relaxed), 0);
    }
}
