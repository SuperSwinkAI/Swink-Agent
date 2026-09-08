//! Tests for `store`.
#![cfg(test)]

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
    fn save(&self, _id: &str, _meta: &SessionMeta, _messages: &[AgentMessage]) -> io::Result<()> {
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
    fn save(&self, _id: &str, _meta: &SessionMeta, _messages: &[AgentMessage]) -> io::Result<()> {
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
