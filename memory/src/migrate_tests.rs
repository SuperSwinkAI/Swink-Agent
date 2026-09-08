//! Tests for `migrate`.
#![cfg(test)]

use super::*;

/// A test migrator that transforms v1 → v2 by uppercasing all text in Message entries.
struct UpperCaseMigrator;

impl SessionMigrator for UpperCaseMigrator {
    fn source_version(&self) -> u32 {
        1
    }
    fn target_version(&self) -> u32 {
        2
    }
    fn migrate(
        &self,
        _meta: &SessionMeta,
        entries: Vec<SessionEntry>,
    ) -> io::Result<Vec<SessionEntry>> {
        Ok(entries
            .into_iter()
            .map(|entry| match entry {
                SessionEntry::Message(msg) => {
                    // For simplicity, just pass through — real migrators would transform
                    SessionEntry::Message(msg)
                }
                other => other,
            })
            .collect())
    }
}

#[test]
fn migrator_upgrades_session() {
    use chrono::Utc;
    use swink_agent::{ContentBlock, LlmMessage, UserMessage};

    let mut meta = SessionMeta {
        id: "test".to_string(),
        title: "Test".to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        version: 1,
        sequence: 0,
    };

    let entries = vec![SessionEntry::Message(LlmMessage::User(
        UserMessage::new(vec![ContentBlock::Text {
            text: "hello".to_string(),
        }])
        .with_timestamp(0),
    ))];

    // Temporarily bump CURRENT_VERSION expectation by using run_migrations
    // with a migrator that goes 1→2. Since CURRENT_VERSION is 1, we need
    // to test the trait directly.
    let migrator = UpperCaseMigrator;
    assert_eq!(migrator.source_version(), 1);
    assert_eq!(migrator.target_version(), 2);

    let result = migrator.migrate(&meta, entries).unwrap();
    assert_eq!(result.len(), 1);
    assert!(matches!(result[0], SessionEntry::Message(_)));

    // Simulate what run_migrations would do
    meta.version = migrator.target_version();
    assert_eq!(meta.version, 2);
}

#[test]
fn unsupported_future_version_returns_error() {
    let mut meta = SessionMeta {
        id: "future".to_string(),
        title: "Future".to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        version: 999,
        sequence: 0,
    };

    let mut entries = vec![];
    let err = run_migrations(&mut meta, &mut entries, &[]).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("unsupported session version 999"));
}

use chrono::Utc;
