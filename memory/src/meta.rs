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
    /// Plugin-defined metadata (tags, embeddings info, intent, etc.).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub custom: serde_json::Value,
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
            custom: serde_json::json!({"tag": "test"}),
        };

        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: SessionMeta = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, meta.id);
        assert_eq!(deserialized.model, meta.model);
        assert_eq!(deserialized.system_prompt, meta.system_prompt);
        assert_eq!(deserialized.created_at, meta.created_at);
        assert_eq!(deserialized.updated_at, meta.updated_at);
        assert_eq!(deserialized.message_count, meta.message_count);
        assert_eq!(deserialized.custom, serde_json::json!({"tag": "test"}));
    }

    #[test]
    fn session_meta_custom_field_roundtrip() {
        let custom = serde_json::json!({
            "tags": ["important", "reviewed"],
            "embedding_model": "text-embedding-3-small",
            "intent": "code_review",
        });
        let meta = SessionMeta {
            id: "20250315_130000".to_string(),
            model: "claude-sonnet".to_string(),
            system_prompt: "Be concise.".to_string(),
            created_at: 1_710_500_200,
            updated_at: 1_710_500_300,
            message_count: 10,
            custom: custom.clone(),
        };

        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: SessionMeta = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.custom, custom);
        assert_eq!(deserialized.custom["tags"][0], "important");
        assert_eq!(deserialized.custom["intent"], "code_review");
    }

    #[test]
    fn session_meta_backward_compat_no_custom_field() {
        // JSON without the `custom` field should deserialize with Value::Null.
        let json = r#"{
            "id": "20250315_140000",
            "model": "gpt-4o",
            "system_prompt": "Hello.",
            "created_at": 1710500400,
            "updated_at": 1710500500,
            "message_count": 3
        }"#;

        let meta: SessionMeta = serde_json::from_str(json).unwrap();
        assert!(meta.custom.is_null());

        // Re-serializing should omit the `custom` field (skip_serializing_if).
        let reserialized = serde_json::to_string(&meta).unwrap();
        assert!(!reserialized.contains("custom"));
    }
}
