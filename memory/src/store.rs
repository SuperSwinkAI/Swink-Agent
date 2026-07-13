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
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use chrono::Utc;
    use serde_json::json;

    use super::*;

    struct CountingStore {
        saves: Arc<AtomicUsize>,
        state_saves: Arc<AtomicUsize>,
        loads: Arc<AtomicUsize>,
        state_loads: Arc<AtomicUsize>,
    }

    impl SessionStore for CountingStore {
        fn save(
            &self,
            _id: &str,
            _meta: &SessionMeta,
            _messages: &[AgentMessage],
        ) -> io::Result<()> {
            self.saves.fetch_add(1, Ordering::Relaxed);
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
            self.loads.fetch_add(1, Ordering::Relaxed);
            Ok((sample_meta(), Vec::new()))
        }

        fn list(&self) -> io::Result<Vec<SessionMeta>> {
            Ok(Vec::new())
        }

        fn delete(&self, _id: &str) -> io::Result<()> {
            Ok(())
        }

        fn save_state(&self, _id: &str, _state: &serde_json::Value) -> io::Result<()> {
            self.state_saves.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn load_state(&self, _id: &str) -> io::Result<Option<serde_json::Value>> {
            self.state_loads.fetch_add(1, Ordering::Relaxed);
            Ok(None)
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

    /// Store that relies on the trait defaults for `save_state`/`load_state`,
    /// simulating a pre-034 custom `SessionStore` implementation.
    struct DefaultStateStore;

    impl SessionStore for DefaultStateStore {
        fn save(
            &self,
            _id: &str,
            _meta: &SessionMeta,
            _messages: &[AgentMessage],
        ) -> io::Result<()> {
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

        fn load_with_options(
            &self,
            _id: &str,
            _options: &LoadOptions,
        ) -> io::Result<(SessionMeta, Vec<SessionEntry>)> {
            Ok((sample_meta(), Vec::new()))
        }
    }

    /// Captures tracing output into a shared buffer for assertions.
    #[derive(Clone, Default)]
    struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

    impl CaptureWriter {
        fn contents(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl std::io::Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// The default `save_state`/`load_state` must stay behavioral no-ops
    /// (spec 034 FR-018 / SC-006) while warning exactly once per process
    /// per method that state persistence is not implemented.
    ///
    /// This is the only test in this binary that exercises the defaults
    /// (all other tests use stores that override the state methods), so the
    /// process-wide `Once` observed here is deterministic.
    #[test]
    fn default_state_methods_are_noops_and_warn_once_per_process() {
        let store = DefaultStateStore;
        let writer = CaptureWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .with_ansi(false)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            // No-op contract preserved: save succeeds, load returns None.
            store
                .save_state("session-1", &json!({"scroll": 1}))
                .unwrap();
            assert_eq!(store.load_state("session-1").unwrap(), None);

            // Second round trips must not warn again.
            store
                .save_state("session-1", &json!({"scroll": 2}))
                .unwrap();
            assert_eq!(store.load_state("session-1").unwrap(), None);
        });

        let output = writer.contents();
        assert!(
            output.contains("WARN"),
            "expected WARN level output: {output}"
        );
        assert_eq!(
            output
                .matches("SessionStore::save_state default no-op")
                .count(),
            1,
            "save_state warning must fire exactly once: {output}"
        );
        assert_eq!(
            output
                .matches("SessionStore::load_state default no-op")
                .count(),
            1,
            "load_state warning must fire exactly once: {output}"
        );
    }

    #[test]
    fn default_save_full_rejects_non_atomic_fallback_without_writing() {
        let save_calls = Arc::new(AtomicUsize::new(0));
        let save_state_calls = Arc::new(AtomicUsize::new(0));
        let store = CountingStore {
            saves: Arc::clone(&save_calls),
            state_saves: Arc::clone(&save_state_calls),
            loads: Arc::new(AtomicUsize::new(0)),
            state_loads: Arc::new(AtomicUsize::new(0)),
        };

        let error = store
            .save_full("session-1", &sample_meta(), &[], &json!({"draft": true}))
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert_eq!(save_calls.load(Ordering::Relaxed), 0);
        assert_eq!(save_state_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn default_load_full_rejects_non_atomic_fallback_without_reading() {
        let load_calls = Arc::new(AtomicUsize::new(0));
        let load_state_calls = Arc::new(AtomicUsize::new(0));
        let store = CountingStore {
            saves: Arc::new(AtomicUsize::new(0)),
            state_saves: Arc::new(AtomicUsize::new(0)),
            loads: Arc::clone(&load_calls),
            state_loads: Arc::clone(&load_state_calls),
        };

        let error = store.load_full("session-1", None).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert_eq!(load_calls.load(Ordering::Relaxed), 0);
        assert_eq!(load_state_calls.load(Ordering::Relaxed), 0);
    }
}
