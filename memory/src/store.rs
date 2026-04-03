//! Session storage trait for pluggable persistence backends.

use std::io;

use swink_agent::{AgentMessage, CustomMessageRegistry, LlmMessage};

use crate::entry::SessionEntry;
use crate::interrupt::InterruptState;
use crate::load_options::LoadOptions;
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

    /// Persist a session including both LLM and custom messages.
    ///
    /// The default implementation filters to `LlmMessage` only and delegates
    /// to [`save`](Self::save).
    fn save_full(&self, id: &str, meta: &SessionMeta, messages: &[AgentMessage]) -> io::Result<()> {
        let llm_messages: Vec<LlmMessage> = messages
            .iter()
            .filter_map(|m| match m {
                AgentMessage::Llm(llm) => Some(llm.clone()),
                AgentMessage::Custom(_) => None,
            })
            .collect();
        self.save(id, meta, &llm_messages)
    }

    /// Load a session including custom messages.
    ///
    /// If `registry` is `Some`, custom messages are deserialized using the
    /// provided registry. The default implementation delegates to
    /// [`load`](Self::load) and wraps each `LlmMessage` in
    /// `AgentMessage::Llm`.
    fn load_full(
        &self,
        id: &str,
        registry: Option<&CustomMessageRegistry>,
    ) -> io::Result<(SessionMeta, Vec<AgentMessage>)> {
        let _ = registry; // unused in default impl
        let (meta, llm_messages) = self.load(id)?;
        let messages = llm_messages.into_iter().map(AgentMessage::Llm).collect();
        Ok((meta, messages))
    }

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
