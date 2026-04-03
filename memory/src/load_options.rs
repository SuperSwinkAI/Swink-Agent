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
/// let options = LoadOptions {
///     last_n_entries: Some(10),
///     ..Default::default()
/// };
/// let (meta, entries) = store.load_with_options("session_id", &options)?;
/// ```
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

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<LoadOptions>();
};
