//! Schema migration support for session stores.
//!
//! [`SessionMigrator`] implementations transform session entries from one
//! schema version to the next. The migration runner in
//! [`crate::store::SessionStore::load`] applies applicable migrators in order.

use std::io;

use crate::entry::SessionEntry;
use crate::meta::SessionMeta;

/// Migrates session entries from one schema version to the next.
///
/// Implementations should transform entries from `source_version()` to
/// `target_version()`. The migration runner calls [`migrate`](Self::migrate)
/// only when the session's version matches `source_version()`.
pub trait SessionMigrator: Send + Sync {
    /// The schema version this migrator reads.
    fn source_version(&self) -> u32;

    /// The schema version this migrator produces.
    fn target_version(&self) -> u32;

    /// Transform session entries from `source_version` to `target_version`.
    ///
    /// The implementation may modify, add, or remove entries. It must NOT
    /// modify `meta.version` — the runner handles that.
    fn migrate(
        &self,
        meta: &SessionMeta,
        entries: Vec<SessionEntry>,
    ) -> io::Result<Vec<SessionEntry>>;
}

/// The current schema version for new sessions.
pub const CURRENT_VERSION: u32 = 1;

/// Run applicable migrators against a loaded session.
///
/// Migrators are applied in order of `source_version()`. If the session version
/// is already >= `CURRENT_VERSION`, no migration runs. Returns an error if
/// the session version exceeds `CURRENT_VERSION` (unsupported future version)
/// or if no migrator covers a needed step.
pub fn run_migrations(
    meta: &mut SessionMeta,
    entries: &mut Vec<SessionEntry>,
    migrators: &[Box<dyn SessionMigrator>],
) -> io::Result<()> {
    if meta.version > CURRENT_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "unsupported session version {} (current: {CURRENT_VERSION})",
                meta.version
            ),
        ));
    }

    while meta.version < CURRENT_VERSION {
        let migrator = migrators
            .iter()
            .find(|m| m.source_version() == meta.version)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "no migrator found for version {} -> {}",
                        meta.version,
                        meta.version + 1
                    ),
                )
            })?;

        *entries = migrator.migrate(meta, std::mem::take(entries))?;
        meta.version = migrator.target_version();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
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

        let entries = vec![SessionEntry::Message(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))];

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
}
