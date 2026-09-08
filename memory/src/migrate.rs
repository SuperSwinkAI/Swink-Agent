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
#[path = "migrate_tests.rs"]
mod tests;
