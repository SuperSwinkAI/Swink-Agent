//! Session storage trait for pluggable persistence backends.

use std::io;

use swink_agent::LlmMessage;

use crate::meta::SessionMeta;

/// Pluggable session persistence.
///
/// Implementations store and retrieve conversation sessions. The default
/// implementation ([`JsonlSessionStore`](crate::JsonlSessionStore)) uses
/// JSONL files, but alternative backends (`SQLite`, S3, etc.) can implement
/// this trait directly.
pub trait SessionStore: Send + Sync {
    /// Persist a session. Overwrites any existing session with the same ID.
    fn save(&self, id: &str, meta: &SessionMeta, messages: &[LlmMessage]) -> io::Result<()>;

    /// Append messages to an existing session without rewriting the entire file.
    fn append(&self, id: &str, messages: &[LlmMessage]) -> io::Result<()>;

    /// Load a session by ID, returning metadata and messages.
    fn load(&self, id: &str) -> io::Result<(SessionMeta, Vec<LlmMessage>)>;

    /// List all saved sessions, sorted by last updated (newest first).
    fn list(&self) -> io::Result<Vec<SessionMeta>>;

    /// Delete a session by ID.
    fn delete(&self, id: &str) -> io::Result<()>;
}
