//! Integration tests for session save/load round-trips (US1, US4, US6).

mod common;

use std::io;

use chrono::DateTime;
use swink_agent::{LlmMessage, ModelSpec};
use swink_agent_memory::{JsonlSessionStore, SessionEntry, SessionMeta, SessionStore};

use common::{assistant_message, sample_meta, sample_meta_with_times, user_message};

// --- US1: Save and Load ---

#[test]
fn save_and_load_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("test_001", "Test session");
    let messages = vec![user_message("hello"), assistant_message("hi there")];

    store.save("test_001", &meta, &messages).unwrap();

    let (loaded_meta, loaded_msgs) = store.load("test_001").unwrap();
    assert_eq!(loaded_meta, meta);
    assert_eq!(loaded_msgs.len(), 2);
}

#[test]
fn save_overwrites_existing_session() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta1 = sample_meta("overwrite_test", "Version 1");
    let msgs1 = vec![user_message("first")];
    store.save("overwrite_test", &meta1, &msgs1).unwrap();

    let meta2 = SessionMeta {
        title: "Version 2".to_string(),
        ..meta1
    };
    let msgs2 = vec![user_message("second"), user_message("third")];
    store.save("overwrite_test", &meta2, &msgs2).unwrap();

    let (loaded_meta, loaded_msgs) = store.load("overwrite_test").unwrap();
    assert_eq!(loaded_meta.title, "Version 2");
    assert_eq!(loaded_msgs.len(), 2);
}

#[test]
fn load_nonexistent_session_returns_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let err = store.load("nonexistent").unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
}

#[test]
fn load_empty_file_returns_invalid_data() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    // Create an empty .jsonl file
    std::fs::write(tmp.path().join("empty.jsonl"), "").unwrap();

    let err = store.load("empty").unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn save_and_load_preserves_message_content() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("content_test", "Content test");
    let messages = vec![
        user_message("What is Rust?"),
        assistant_message("Rust is a systems programming language."),
        user_message("Tell me more."),
    ];

    store.save("content_test", &meta, &messages).unwrap();
    let (_, loaded_msgs) = store.load("content_test").unwrap();

    assert_eq!(loaded_msgs.len(), 3);
    // Verify content is preserved by checking serialization roundtrip
    for (orig, loaded) in messages.iter().zip(loaded_msgs.iter()) {
        let orig_json = serde_json::to_string(orig).unwrap();
        let loaded_json = serde_json::to_string(loaded).unwrap();
        assert_eq!(orig_json, loaded_json);
    }
}

#[test]
fn invalid_session_id_rejected_on_save() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();
    let meta = sample_meta("bad", "Bad");

    for bad_id in &["foo/bar", "foo\\bar", "..secret", "null\0byte", ""] {
        let err = store.save(bad_id, &meta, &[]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "id={bad_id:?}");
    }
}

#[test]
fn invalid_session_id_rejected_on_load() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    for bad_id in &["foo/bar", "foo\\bar", "..secret", "null\0byte", ""] {
        let err = store.load(bad_id).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "id={bad_id:?}");
    }
}

// --- US4: List and Delete ---

#[test]
fn list_returns_all_sessions_with_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    for (id, title) in &[("s1", "First"), ("s2", "Second"), ("s3", "Third")] {
        let meta = sample_meta(id, title);
        store.save(id, &meta, &[]).unwrap();
    }

    let sessions = store.list().unwrap();
    assert_eq!(sessions.len(), 3);

    let titles: Vec<&str> = sessions.iter().map(|s| s.title.as_str()).collect();
    assert!(titles.contains(&"First"));
    assert!(titles.contains(&"Second"));
    assert!(titles.contains(&"Third"));
}

#[test]
fn list_sorted_by_most_recent() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let old_time = DateTime::from_timestamp(1_000_000, 0).unwrap().to_utc();
    let new_time = DateTime::from_timestamp(2_000_000, 0).unwrap().to_utc();

    let meta_old = sample_meta_with_times("old", "Old session", old_time, old_time);
    let meta_new = sample_meta_with_times("new", "New session", new_time, new_time);

    store.save("old", &meta_old, &[]).unwrap();
    store.save("new", &meta_new, &[]).unwrap();

    let sessions = store.list().unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].id, "new");
    assert_eq!(sessions[1].id, "old");
}

#[test]
fn delete_removes_session() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("to_delete", "Delete me");
    store.save("to_delete", &meta, &[]).unwrap();

    store.delete("to_delete").unwrap();

    let err = store.load("to_delete").unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::NotFound);

    let sessions = store.list().unwrap();
    assert!(sessions.is_empty());
}

#[test]
fn list_empty_store_returns_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let sessions = store.list().unwrap();
    assert!(sessions.is_empty());
}

// --- US6: Rich Session Entry Types ---

#[test]
fn rich_entries_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("rich_001", "Rich entries test");
    let entries = vec![
        SessionEntry::Message(user_message("hello")),
        SessionEntry::ModelChange {
            from: ModelSpec {
                provider: "openai".to_string(),
                model_id: "gpt-4".to_string(),
                ..ModelSpec::new("", "")
            },
            to: ModelSpec {
                provider: "anthropic".to_string(),
                model_id: "claude-3".to_string(),
                ..ModelSpec::new("", "")
            },
            timestamp: 100,
        },
        SessionEntry::Label {
            text: "important point".to_string(),
            message_index: 0,
            timestamp: 200,
        },
        SessionEntry::Custom {
            type_name: "my_event".to_string(),
            data: serde_json::json!({"key": "value"}),
            timestamp: 300,
        },
    ];

    store.save_entries("rich_001", &meta, &entries).unwrap();
    let (loaded_meta, loaded_entries) = store.load_entries("rich_001").unwrap();

    assert_eq!(loaded_meta, meta);
    assert_eq!(loaded_entries.len(), 4);
    assert!(matches!(
        loaded_entries[0],
        SessionEntry::Message(LlmMessage::User(_))
    ));
    assert!(matches!(
        loaded_entries[1],
        SessionEntry::ModelChange { .. }
    ));
    assert!(matches!(loaded_entries[2], SessionEntry::Label { .. }));
    assert!(matches!(loaded_entries[3], SessionEntry::Custom { .. }));

    // Verify data preserved
    if let SessionEntry::ModelChange {
        from,
        to,
        timestamp,
    } = &loaded_entries[1]
    {
        assert_eq!(from.provider, "openai");
        assert_eq!(to.provider, "anthropic");
        assert_eq!(*timestamp, 100);
    }

    // load() (LlmMessage-only) should return only the Message entry
    let (_, messages) = store.load("rich_001").unwrap();
    assert_eq!(messages.len(), 1);
}

#[test]
fn rich_entries_backward_compat() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    // Create an old-format JSONL file: meta line + raw LlmMessage lines (no entry_type)
    let meta = sample_meta("compat_001", "Old format session");
    let msg1 = user_message("first message");
    let msg2 = assistant_message("second message");

    let meta_json = serde_json::to_string(&meta).unwrap();
    let msg1_json = serde_json::to_string(&msg1).unwrap();
    let msg2_json = serde_json::to_string(&msg2).unwrap();

    let file_content = format!("{meta_json}\n{msg1_json}\n{msg2_json}\n");
    std::fs::write(tmp.path().join("compat_001.jsonl"), file_content).unwrap();

    // load_entries should interpret old-format lines as SessionEntry::Message
    let (loaded_meta, entries) = store.load_entries("compat_001").unwrap();
    assert_eq!(loaded_meta, meta);
    assert_eq!(entries.len(), 2);
    assert!(matches!(
        entries[0],
        SessionEntry::Message(LlmMessage::User(_))
    ));
    assert!(matches!(
        entries[1],
        SessionEntry::Message(LlmMessage::Assistant(_))
    ));
}
