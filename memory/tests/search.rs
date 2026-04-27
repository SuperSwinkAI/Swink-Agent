//! Integration tests for cross-session full-text search.
//!
//! Tests gated on the `search` feature exercise the tantivy-backed index.
//! Tests without the gate exercise the baseline behaviour (empty default impl
//! on the trait, or the linear scan in `JsonlSessionStore`).

mod common;

use swink_agent_memory::{JsonlSessionStore, SessionEntry, SessionSearchOptions, SessionStore};

// ─── helpers ─────────────────────────────────────────────────────────────────

fn make_store() -> (JsonlSessionStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).expect("store");
    (store, dir)
}

fn save_session_with_text(
    store: &JsonlSessionStore,
    session_id: &str,
    title: &str,
    texts: &[&str],
) {
    let meta = common::sample_meta(session_id, title);
    let entries: Vec<SessionEntry> = texts
        .iter()
        .map(|t| SessionEntry::Message(common::user_message_at(t, 1_000)))
        .collect();
    store
        .save_entries(session_id, &meta, &entries)
        .expect("save_entries");
}

// ─── baseline behaviour (no feature gate) ────────────────────────────────────

/// Default `SessionStore::search()` returns empty vec (backward-compat no-op).
///
/// This exercises the trait default; the `JsonlSessionStore` overrides it, but
/// any custom `SessionStore` impl that does NOT override it should return `[]`.
#[test]
fn default_trait_search_returns_empty() {
    use std::io;
    use swink_agent::AgentMessage;
    use swink_agent::CustomMessageRegistry;
    use swink_agent_memory::{LoadOptions, SessionMeta};

    struct NoOpStore;
    impl SessionStore for NoOpStore {
        fn save(
            &self,
            _id: &str,
            _meta: &SessionMeta,
            _messages: &[AgentMessage],
        ) -> io::Result<()> {
            Ok(())
        }
        fn append(&self, _id: &str, _messages: &[AgentMessage]) -> io::Result<()> {
            Ok(())
        }
        fn load(
            &self,
            _id: &str,
            _registry: Option<&CustomMessageRegistry>,
        ) -> io::Result<(SessionMeta, Vec<AgentMessage>)> {
            Err(io::Error::from(io::ErrorKind::NotFound))
        }
        fn list(&self) -> io::Result<Vec<SessionMeta>> {
            Ok(vec![])
        }
        fn delete(&self, _id: &str) -> io::Result<()> {
            Ok(())
        }
        fn load_with_options(
            &self,
            _id: &str,
            _options: &LoadOptions,
        ) -> io::Result<(SessionMeta, Vec<SessionEntry>)> {
            Err(io::Error::from(io::ErrorKind::NotFound))
        }
    }

    let store = NoOpStore;
    let hits = store
        .search("anything", &SessionSearchOptions::default())
        .expect("search");
    assert!(hits.is_empty(), "default no-op should return empty vec");
}

/// Search on an empty `JsonlSessionStore` returns empty vec.
#[test]
fn jsonl_search_empty_store_returns_empty() {
    let (store, _dir) = make_store();
    let hits = store
        .search("hello", &SessionSearchOptions::default())
        .expect("search");
    assert!(hits.is_empty());
}

/// After saving entries, search finds relevant content.
#[test]
fn jsonl_search_finds_saved_content() {
    let (store, _dir) = make_store();

    save_session_with_text(
        &store,
        "session-alpha",
        "Alpha Session",
        &["the quick brown fox"],
    );
    save_session_with_text(
        &store,
        "session-beta",
        "Beta Session",
        &["lazy dog sits here"],
    );

    let hits = store
        .search("quick fox", &SessionSearchOptions::default())
        .expect("search");

    assert!(
        !hits.is_empty(),
        "should find at least one hit for 'quick fox'"
    );
    assert!(
        hits.iter().any(|h| h.session_id == "session-alpha"),
        "hit should be from session-alpha"
    );
    assert!(
        hits.iter().all(|h| h.session_id != "session-beta"),
        "session-beta should not match 'quick fox'"
    );
}

/// `max_results` option is respected.
#[test]
fn jsonl_search_respects_max_results() {
    let (store, _dir) = make_store();

    // Create 5 sessions each containing the word "apple".
    for i in 0..5u32 {
        let id = format!("session-{i}");
        save_session_with_text(&store, &id, &format!("Session {i}"), &["apple pie recipe"]);
    }

    let opts = SessionSearchOptions {
        max_results: Some(2),
        ..Default::default()
    };
    let hits = store.search("apple", &opts).expect("search");
    assert!(
        hits.len() <= 2,
        "should return at most 2 hits, got {}",
        hits.len()
    );
}

/// Session ID filter restricts search to specified sessions.
#[test]
fn jsonl_search_session_id_filter() {
    let (store, _dir) = make_store();

    save_session_with_text(
        &store,
        "target-session",
        "Target",
        &["needle in a haystack"],
    );
    save_session_with_text(&store, "other-session", "Other", &["needle found here too"]);

    let opts = SessionSearchOptions {
        session_ids: Some(vec!["target-session".to_string()]),
        ..Default::default()
    };
    let hits = store.search("needle", &opts).expect("search");
    assert!(
        hits.iter().all(|h| h.session_id == "target-session"),
        "all hits should be from target-session"
    );
}

/// Entry type filter only returns matching entry types.
#[test]
fn jsonl_search_entry_type_filter_excludes_non_message_types() {
    let (store, _dir) = make_store();

    let meta = common::sample_meta("mixed-session", "Mixed");
    let entries = vec![
        SessionEntry::Message(common::user_message_at("unique phrase here", 1_000)),
        SessionEntry::Label {
            text: "unique phrase label".to_string(),
            message_index: 0,
            timestamp: 2_000,
        },
    ];
    store
        .save_entries("mixed-session", &meta, &entries)
        .expect("save_entries");

    // Restrict to only "message" entries.
    let opts = SessionSearchOptions {
        entry_types: Some(vec!["message".to_string()]),
        ..Default::default()
    };
    let hits = store.search("unique phrase", &opts).expect("search");
    assert!(
        hits.iter().all(|h| h.entry.entry_type_name() == "message"),
        "all hits should be message entries"
    );
}

// ─── tantivy-specific tests (require `search` feature) ───────────────────────

#[cfg(feature = "search")]
mod tantivy_tests {
    use super::*;
    use swink_agent_memory::TantivyIndex;

    /// Tantivy index can be opened and populated from a store.
    #[test]
    fn tantivy_index_open_and_search() {
        let (store, _dir) = make_store();

        save_session_with_text(
            &store,
            "idx-session-1",
            "Index Session 1",
            &["tantivy full text search works"],
        );
        save_session_with_text(
            &store,
            "idx-session-2",
            "Index Session 2",
            &["unrelated content about gardening"],
        );

        let hits = store
            .search("tantivy full text", &SessionSearchOptions::default())
            .expect("search");

        assert!(!hits.is_empty(), "should find hits for 'tantivy full text'");
        assert!(
            hits.iter().any(|h| h.session_id == "idx-session-1"),
            "idx-session-1 should appear in results"
        );
    }

    /// After building the index once, subsequent searches reuse it without rebuild.
    #[test]
    fn tantivy_second_search_does_not_rebuild() {
        let (store, _dir) = make_store();
        save_session_with_text(&store, "warm-session", "Warm", &["warm cache test phrase"]);

        // First search builds the index.
        let hits1 = store
            .search("warm cache", &SessionSearchOptions::default())
            .expect("first search");

        // Second search should return the same results.
        let hits2 = store
            .search("warm cache", &SessionSearchOptions::default())
            .expect("second search");

        assert_eq!(
            hits1.len(),
            hits2.len(),
            "repeated search should return the same number of hits"
        );
    }

    /// `rebuild_search_index` re-indexes all current sessions.
    #[test]
    fn rebuild_search_index_repopulates() {
        let (store, _dir) = make_store();

        save_session_with_text(
            &store,
            "rebuild-session",
            "Rebuild Session",
            &["rebuild index test phrase"],
        );

        store.rebuild_search_index().expect("rebuild_search_index");

        let hits = store
            .search("rebuild index", &SessionSearchOptions::default())
            .expect("search after rebuild");

        assert!(
            !hits.is_empty(),
            "should find hits after explicit index rebuild"
        );
    }

    /// `TantivyIndex::search` on an empty index returns an empty vec.
    #[test]
    fn tantivy_index_search_empty_returns_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index = TantivyIndex::open_or_create(dir.path()).expect("open_or_create");
        let hits = index
            .search("anything", &SessionSearchOptions::default())
            .expect("search");
        assert!(hits.is_empty());
    }

    /// `TantivyIndex` respects max_results.
    #[test]
    fn tantivy_index_respects_max_results() {
        let (store, _dir) = make_store();

        for i in 0..5u32 {
            let id = format!("tantivy-max-{i}");
            save_session_with_text(&store, &id, &format!("Session {i}"), &["mango smoothie"]);
        }

        let opts = SessionSearchOptions {
            max_results: Some(2),
            ..Default::default()
        };
        let hits = store.search("mango", &opts).expect("search");
        assert!(
            hits.len() <= 2,
            "tantivy search should respect max_results=2, got {}",
            hits.len()
        );
    }
}
