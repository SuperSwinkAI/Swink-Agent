//! Session metadata stored alongside persisted conversations.

use serde::{Deserialize, Serialize};

/// Metadata stored as the first line of a session JSONL file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    /// Unique session identifier (timestamp-based).
    pub id: String,
    /// Model identifier used in this session.
    pub model: String,
    /// System prompt for the session.
    pub system_prompt: String,
    /// Unix timestamp when the session was created.
    pub created_at: u64,
    /// Unix timestamp when the session was last updated.
    pub updated_at: u64,
    /// Number of LLM messages in the session.
    pub message_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization_roundtrip() {
        let meta = SessionMeta {
            id: "20250315_120000".to_string(),
            model: "gpt-4o".to_string(),
            system_prompt: "You are helpful.".to_string(),
            created_at: 1_710_500_000,
            updated_at: 1_710_500_100,
            message_count: 5,
        };

        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: SessionMeta = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, meta.id);
        assert_eq!(deserialized.model, meta.model);
        assert_eq!(deserialized.system_prompt, meta.system_prompt);
        assert_eq!(deserialized.created_at, meta.created_at);
        assert_eq!(deserialized.updated_at, meta.updated_at);
        assert_eq!(deserialized.message_count, meta.message_count);
    }
}
