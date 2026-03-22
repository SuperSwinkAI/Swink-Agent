//! Session metadata stored alongside persisted conversations.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Metadata stored as the first line of a session JSONL file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMeta {
    /// Unique session identifier.
    pub id: String,
    /// Human-readable session title.
    pub title: String,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// When the session was last updated.
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization_roundtrip() {
        let meta = SessionMeta {
            id: "20250315_120000".to_string(),
            title: "Test session".to_string(),
            created_at: DateTime::from_timestamp(1_710_500_000, 0).unwrap().to_utc(),
            updated_at: DateTime::from_timestamp(1_710_500_100, 0).unwrap().to_utc(),
        };

        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: SessionMeta = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized, meta);
    }

    #[test]
    fn partial_eq_works() {
        let a = SessionMeta {
            id: "a".to_string(),
            title: "Session A".to_string(),
            created_at: DateTime::from_timestamp(100, 0).unwrap().to_utc(),
            updated_at: DateTime::from_timestamp(200, 0).unwrap().to_utc(),
        };
        let b = a.clone();
        assert_eq!(a, b);
    }
}
