//! Session storage trait for pluggable persistence backends.

use std::io;

use swink_agent::AgentMessage;

use crate::meta::SessionMeta;

/// Filter criteria for session listing.
#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    /// Filter by model name (substring match).
    pub model: Option<String>,
    /// Only sessions created after this timestamp.
    pub created_after: Option<u64>,
    /// Only sessions updated after this timestamp.
    pub updated_after: Option<u64>,
    /// Minimum message count.
    pub min_messages: Option<usize>,
}

impl SessionFilter {
    /// Check if a session matches this filter.
    pub fn matches(&self, meta: &SessionMeta) -> bool {
        if let Some(ref model) = self.model
            && !meta.model.contains(model)
        {
            return false;
        }
        if let Some(after) = self.created_after
            && meta.created_at <= after
        {
            return false;
        }
        if let Some(after) = self.updated_after
            && meta.updated_at <= after
        {
            return false;
        }
        if let Some(min) = self.min_messages
            && meta.message_count < min
        {
            return false;
        }
        true
    }
}

/// Pluggable session persistence.
///
/// Implementations store and retrieve conversation sessions. The default
/// implementation ([`JsonlSessionStore`](crate::JsonlSessionStore)) uses
/// JSONL files, but alternative backends (`SQLite`, S3, etc.) can implement
/// this trait directly.
pub trait SessionStore: Send + Sync {
    /// Persist a session. Overwrites any existing session with the same ID.
    fn save(
        &self,
        id: &str,
        model: &str,
        system_prompt: &str,
        messages: &[AgentMessage],
    ) -> io::Result<()>;

    /// Load a session by ID, returning metadata and messages.
    fn load(&self, id: &str) -> io::Result<(SessionMeta, Vec<AgentMessage>)>;

    /// List all saved sessions, sorted by last updated (newest first).
    fn list(&self) -> io::Result<Vec<SessionMeta>>;

    /// Delete a session by ID.
    fn delete(&self, id: &str) -> io::Result<()>;

    /// Generate a new unique session ID.
    fn new_session_id(&self) -> String;

    /// List sessions matching the given filter, sorted by last updated (newest first).
    ///
    /// Default implementation calls [`list()`](Self::list) and filters in-memory.
    fn list_filtered(&self, filter: &SessionFilter) -> io::Result<Vec<SessionMeta>> {
        let all = self.list()?;
        Ok(all.into_iter().filter(|m| filter.matches(m)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_meta(
        id: &str,
        model: &str,
        created_at: u64,
        updated_at: u64,
        count: usize,
    ) -> SessionMeta {
        SessionMeta {
            id: id.to_string(),
            model: model.to_string(),
            system_prompt: "prompt".to_string(),
            created_at,
            updated_at,
            message_count: count,
            custom: serde_json::Value::Null,
        }
    }

    #[test]
    fn session_filter_matches_model() {
        let meta = sample_meta("1", "claude-sonnet-4-20250514", 100, 200, 5);

        let filter = SessionFilter {
            model: Some("claude".to_string()),
            ..Default::default()
        };
        assert!(filter.matches(&meta));

        let filter = SessionFilter {
            model: Some("gpt".to_string()),
            ..Default::default()
        };
        assert!(!filter.matches(&meta));

        // Substring match
        let filter = SessionFilter {
            model: Some("sonnet".to_string()),
            ..Default::default()
        };
        assert!(filter.matches(&meta));
    }

    #[test]
    fn session_filter_matches_date_range() {
        let meta = sample_meta("1", "model", 100, 200, 5);

        // created_after: session created_at must be > threshold
        let filter = SessionFilter {
            created_after: Some(50),
            ..Default::default()
        };
        assert!(filter.matches(&meta));

        let filter = SessionFilter {
            created_after: Some(100),
            ..Default::default()
        };
        assert!(!filter.matches(&meta)); // 100 <= 100

        // updated_after
        let filter = SessionFilter {
            updated_after: Some(150),
            ..Default::default()
        };
        assert!(filter.matches(&meta));

        let filter = SessionFilter {
            updated_after: Some(200),
            ..Default::default()
        };
        assert!(!filter.matches(&meta)); // 200 <= 200
    }

    #[test]
    fn session_filter_matches_min_messages() {
        let meta = sample_meta("1", "model", 100, 200, 5);

        let filter = SessionFilter {
            min_messages: Some(5),
            ..Default::default()
        };
        assert!(filter.matches(&meta));

        let filter = SessionFilter {
            min_messages: Some(6),
            ..Default::default()
        };
        assert!(!filter.matches(&meta));
    }

    #[test]
    fn session_filter_default_matches_all() {
        let filter = SessionFilter::default();
        let meta = sample_meta("1", "any-model", 0, 0, 0);
        assert!(filter.matches(&meta));
    }
}
