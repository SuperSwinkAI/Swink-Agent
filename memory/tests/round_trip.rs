//! Integration tests for session save/load round-trips (US1, US4, US6, US7, US8, US9).

mod common;

use std::io;

use chrono::DateTime;
use swink_agent::{LlmMessage, ModelSpec};
use swink_agent_memory::{
    InterruptState, JsonlSessionStore, LoadOptions, PendingToolCall, SessionEntry, SessionMeta,
    SessionStore,
};

use common::{
    assistant_message, llm_assistant_message, llm_user_message, sample_meta,
    sample_meta_with_times, user_message, user_message_at,
};

// --- US1: Save and Load ---

#[test]
fn save_and_load_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("test_001", "Test session");
    let messages = vec![user_message("hello"), assistant_message("hi there")];

    store.save("test_001", &meta, &messages).unwrap();

    let (loaded_meta, loaded_msgs) = store.load("test_001", None).unwrap();
    assert_eq!(loaded_meta.id, meta.id);
    assert_eq!(loaded_meta.title, meta.title);
    assert_eq!(loaded_meta.version, 1);
    assert_eq!(loaded_meta.sequence, 1); // incremented on save
    assert_eq!(loaded_msgs.len(), 2);
}

#[test]
fn save_overwrites_existing_session() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta1 = sample_meta("overwrite_test", "Version 1");
    let msgs1 = vec![user_message("first")];
    store.save("overwrite_test", &meta1, &msgs1).unwrap();

    // Load to get updated sequence after first save
    let (saved_meta, _) = store.load("overwrite_test", None).unwrap();
    let meta2 = SessionMeta {
        title: "Version 2".to_string(),
        ..saved_meta
    };
    let msgs2 = vec![user_message("second"), user_message("third")];
    store.save("overwrite_test", &meta2, &msgs2).unwrap();

    let (loaded_meta, loaded_msgs) = store.load("overwrite_test", None).unwrap();
    assert_eq!(loaded_meta.title, "Version 2");
    assert_eq!(loaded_msgs.len(), 2);
}

#[test]
fn load_nonexistent_session_returns_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let err = store.load("nonexistent", None).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
}

#[test]
fn load_empty_file_returns_invalid_data() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    // Create an empty .jsonl file
    std::fs::write(tmp.path().join("empty.jsonl"), "").unwrap();

    let err = store.load("empty", None).unwrap_err();
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
    let (_, loaded_msgs) = store.load("content_test", None).unwrap();

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
        let err = store.load(bad_id, None).unwrap_err();
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

    let err = store.load("to_delete", None).unwrap_err();
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
        SessionEntry::Message(llm_user_message("hello")),
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

    assert_eq!(loaded_meta.id, meta.id);
    assert_eq!(loaded_meta.title, meta.title);
    assert_eq!(loaded_meta.sequence, 1); // incremented on save
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
    let (_, messages) = store.load("rich_001", None).unwrap();
    assert_eq!(messages.len(), 1);
}

#[test]
fn rich_entries_backward_compat() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    // Create an old-format JSONL file: meta line + raw LlmMessage lines (no entry_type)
    let meta = sample_meta("compat_001", "Old format session");
    let msg1 = llm_user_message("first message");
    let msg2 = llm_assistant_message("second message");

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

// --- US7: Session Versioning ---

#[test]
fn version_defaults_for_old_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    // Write an old-format JSONL file without version/sequence fields
    let meta_json = r#"{"id":"old_001","title":"Old session","created_at":"2025-03-15T12:00:00Z","updated_at":"2025-03-15T12:00:00Z"}"#;
    let msg_json = serde_json::to_string(&llm_user_message("hello")).unwrap();
    let content = format!("{meta_json}\n{msg_json}\n");
    std::fs::write(tmp.path().join("old_001.jsonl"), content).unwrap();

    let (loaded_meta, msgs) = store.load("old_001", None).unwrap();
    assert_eq!(loaded_meta.version, 1); // default
    assert_eq!(loaded_meta.sequence, 0); // default
    assert_eq!(msgs.len(), 1);
}

#[test]
fn sequence_increments_on_save() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("seq_test", "Sequence test");
    store
        .save("seq_test", &meta, &[user_message("first")])
        .unwrap();

    let (loaded1, _) = store.load("seq_test", None).unwrap();
    assert_eq!(loaded1.sequence, 1);

    // Save again with the loaded meta (sequence=1)
    store
        .save("seq_test", &loaded1, &[user_message("second")])
        .unwrap();

    let (loaded2, _) = store.load("seq_test", None).unwrap();
    assert_eq!(loaded2.sequence, 2);
}

#[test]
fn optimistic_concurrency_rejects_stale_sequence() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("conc_test", "Concurrency test");
    store
        .save("conc_test", &meta, &[user_message("v1")])
        .unwrap();

    // Load meta (sequence=1)
    let (stale_meta, _) = store.load("conc_test", None).unwrap();
    assert_eq!(stale_meta.sequence, 1);

    // Simulate another writer saving (bumps sequence to 2)
    store
        .save("conc_test", &stale_meta, &[user_message("v2")])
        .unwrap();

    // Attempt save with stale meta (sequence=1, but file has sequence=2)
    let err = store
        .save("conc_test", &stale_meta, &[user_message("v3")])
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    assert!(err.to_string().contains("sequence conflict"));
}

#[test]
fn unsupported_future_version_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    // Write a JSONL file with version: 999
    let meta_json = r#"{"id":"future_001","title":"Future session","created_at":"2025-03-15T12:00:00Z","updated_at":"2025-03-15T12:00:00Z","version":999,"sequence":0}"#;
    std::fs::write(
        tmp.path().join("future_001.jsonl"),
        format!("{meta_json}\n"),
    )
    .unwrap();

    let err = store.load("future_001", None).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("unsupported session version 999"));
}

// --- US8: Interrupt State Persistence ---

fn sample_interrupt() -> InterruptState {
    InterruptState {
        interrupted_at: 1_710_500_000,
        pending_tool_calls: vec![
            PendingToolCall {
                tool_call_id: "tc_1".to_string(),
                tool_name: "bash".to_string(),
                arguments: serde_json::json!({"command": "ls -la"}),
            },
            PendingToolCall {
                tool_call_id: "tc_2".to_string(),
                tool_name: "read_file".to_string(),
                arguments: serde_json::json!({"path": "/tmp/foo.txt"}),
            },
        ],
        context_snapshot: vec![llm_user_message("hello"), llm_assistant_message("hi")],
        system_prompt: "You are a helpful assistant.".to_string(),
        model: ModelSpec::new("openai", "gpt-4"),
    }
}

#[test]
fn interrupt_save_and_load_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("int_001", "Interrupt test");
    store.save("int_001", &meta, &[user_message("hi")]).unwrap();

    let state = sample_interrupt();
    store.save_interrupt("int_001", &state).unwrap();

    let loaded = store.load_interrupt("int_001").unwrap();
    assert!(loaded.is_some());
    let loaded = loaded.unwrap();
    assert_eq!(loaded.interrupted_at, 1_710_500_000);
    assert_eq!(loaded.pending_tool_calls.len(), 2);
    assert_eq!(loaded.pending_tool_calls[0].tool_name, "bash");
    assert_eq!(loaded.pending_tool_calls[1].tool_name, "read_file");
    assert_eq!(loaded.context_snapshot.len(), 2);
    assert_eq!(loaded.system_prompt, "You are a helpful assistant.");
    assert_eq!(loaded.model.provider, "openai");
    assert_eq!(loaded.model.model_id, "gpt-4");
}

#[test]
fn interrupt_none_when_not_saved() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let result = store.load_interrupt("no_such_session").unwrap();
    assert!(result.is_none());
}

#[test]
fn interrupt_cleared_after_resume() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("int_clear", "Clear test");
    store
        .save("int_clear", &meta, &[user_message("hi")])
        .unwrap();

    let state = sample_interrupt();
    store.save_interrupt("int_clear", &state).unwrap();
    assert!(store.load_interrupt("int_clear").unwrap().is_some());

    store.clear_interrupt("int_clear").unwrap();
    assert!(store.load_interrupt("int_clear").unwrap().is_none());
}

#[test]
fn delete_session_also_deletes_interrupt() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("int_del", "Delete cascade test");
    store.save("int_del", &meta, &[user_message("hi")]).unwrap();

    let state = sample_interrupt();
    store.save_interrupt("int_del", &state).unwrap();

    // Verify interrupt file exists
    assert!(tmp.path().join("int_del.interrupt.json").exists());

    store.delete("int_del").unwrap();

    // Both session and interrupt should be gone
    assert!(!tmp.path().join("int_del.jsonl").exists());
    assert!(!tmp.path().join("int_del.interrupt.json").exists());
}

#[test]
fn corrupted_interrupt_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    // Write garbage to interrupt file
    std::fs::write(
        tmp.path().join("corrupt_int.interrupt.json"),
        "this is not valid json{{{",
    )
    .unwrap();

    let err = store.load_interrupt("corrupt_int").unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
}

// --- US9: Filtered Session Retrieval ---

#[test]
fn load_last_n_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("filter_n", "Last N test");
    let entries: Vec<SessionEntry> = (0..50)
        .map(|i| SessionEntry::Message(user_message_at(&format!("msg_{i}"), i)))
        .collect();

    store.save_entries("filter_n", &meta, &entries).unwrap();

    let options = LoadOptions {
        last_n_entries: Some(10),
        ..Default::default()
    };
    let (_, loaded) = store.load_with_options("filter_n", &options).unwrap();
    assert_eq!(loaded.len(), 10);

    // Verify they are the last 10 entries (timestamps 40..49)
    for (i, entry) in loaded.iter().enumerate() {
        assert_eq!(entry.timestamp(), Some(40 + i as u64));
    }
}

#[test]
fn load_after_timestamp() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("filter_ts", "After timestamp test");
    let entries: Vec<SessionEntry> = (1..=50)
        .map(|i| SessionEntry::Message(user_message_at(&format!("msg_{i}"), i)))
        .collect();

    store.save_entries("filter_ts", &meta, &entries).unwrap();

    let after = DateTime::from_timestamp(25, 0).unwrap().to_utc();
    let options = LoadOptions {
        after_timestamp: Some(after),
        ..Default::default()
    };
    let (_, loaded) = store.load_with_options("filter_ts", &options).unwrap();
    assert_eq!(loaded.len(), 25); // entries with timestamps 26..50

    // Verify all entries have timestamps > 25
    for entry in &loaded {
        assert!(entry.timestamp().unwrap() > 25);
    }
}

#[test]
fn load_by_entry_type() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("filter_type", "Entry type filter test");
    let entries = vec![
        SessionEntry::Message(llm_user_message("hello")),
        SessionEntry::ModelChange {
            from: ModelSpec::new("a", "a"),
            to: ModelSpec::new("b", "b"),
            timestamp: 100,
        },
        SessionEntry::Label {
            text: "bookmark".to_string(),
            message_index: 0,
            timestamp: 200,
        },
        SessionEntry::Message(llm_user_message("world")),
    ];

    store.save_entries("filter_type", &meta, &entries).unwrap();

    let options = LoadOptions {
        entry_types: Some(vec!["message".to_string()]),
        ..Default::default()
    };
    let (_, loaded) = store.load_with_options("filter_type", &options).unwrap();
    assert_eq!(loaded.len(), 2);
    assert!(loaded.iter().all(|e| matches!(e, SessionEntry::Message(_))));
}

#[test]
fn load_options_all_none_returns_full() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("filter_none", "Default options test");
    let entries = vec![
        SessionEntry::Message(llm_user_message("hello")),
        SessionEntry::Label {
            text: "note".to_string(),
            message_index: 0,
            timestamp: 100,
        },
        SessionEntry::Message(llm_user_message("world")),
    ];

    store.save_entries("filter_none", &meta, &entries).unwrap();

    let options = LoadOptions::default();
    let (_, loaded) = store.load_with_options("filter_none", &options).unwrap();
    assert_eq!(loaded.len(), 3);
}
