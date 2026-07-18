//! Session metadata stored alongside persisted conversations.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Metadata stored as the first line of a session JSONL file.
#[non_exhaustive]
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
    /// Schema version for migration support. Defaults to 1.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Optimistic concurrency sequence number. Incremented on every write.
    #[serde(default)]
    pub sequence: u64,
}

const fn default_version() -> u32 {
    1
}

impl SessionMeta {
    /// Creates a new session metadata record with the given identity and timestamps.
    ///
    /// `version` defaults to `1` and `sequence` defaults to `0`; use
    /// [`with_version`](Self::with_version) / [`with_sequence`](Self::with_sequence)
    /// to override either.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            created_at,
            updated_at,
            version: default_version(),
            sequence: 0,
        }
    }

    /// Overrides the schema version (defaults to 1).
    #[must_use]
    pub const fn with_version(mut self, version: u32) -> Self {
        self.version = version;
        self
    }

    /// Overrides the optimistic-concurrency sequence number (defaults to 0).
    #[must_use]
    pub const fn with_sequence(mut self, sequence: u64) -> Self {
        self.sequence = sequence;
        self
    }
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
            version: 1,
            sequence: 0,
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
            version: 1,
            sequence: 0,
        };
        let b = a.clone();
        assert_eq!(a, b);
    }
}
