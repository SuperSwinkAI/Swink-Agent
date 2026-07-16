//! Options for filtered session retrieval.

use chrono::{DateTime, Utc};

/// Options for loading a subset of session entries.
///
/// All fields default to `None`, meaning no filtering is applied and the
/// full session is returned.
///
/// # Examples
///
/// ```rust,ignore
/// let options = LoadOptions::new().with_last_n_entries(10);
/// let (meta, entries) = store.load_with_options("session_id", &options)?;
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Return only the last N entries (applied after other filters).
    pub last_n_entries: Option<usize>,

    /// Return only entries with timestamps after this instant.
    pub after_timestamp: Option<DateTime<Utc>>,

    /// Return only entries whose `entry_type` discriminator matches one of
    /// these strings (e.g., `"message"`, `"model_change"`, `"label"`).
    pub entry_types: Option<Vec<String>>,
}

impl LoadOptions {
    /// Creates a new `LoadOptions` with no filters applied.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Restricts results to the last N entries (applied after other filters).
    #[must_use]
    pub fn with_last_n_entries(mut self, last_n_entries: usize) -> Self {
        self.last_n_entries = Some(last_n_entries);
        self
    }

    /// Restricts results to entries with timestamps after this instant.
    #[must_use]
    pub fn with_after_timestamp(mut self, after_timestamp: DateTime<Utc>) -> Self {
        self.after_timestamp = Some(after_timestamp);
        self
    }

    /// Restricts results to entries whose `entry_type` discriminator matches
    /// one of the given strings (e.g., `"message"`, `"model_change"`, `"label"`).
    #[must_use]
    pub fn with_entry_types(mut self, entry_types: Vec<String>) -> Self {
        self.entry_types = Some(entry_types);
        self
    }
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<LoadOptions>();
};
