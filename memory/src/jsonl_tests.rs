//! Tests for `jsonl`.
#![cfg(test)]

use super::*;
use chrono::Timelike;

#[test]
fn new_session_id_format() {
    let id = JsonlSessionStore::new_session_id();
    let (timestamp, suffix) = id.rsplit_once('_').unwrap();
    assert_eq!(timestamp.len(), 15);
    assert_eq!(timestamp.as_bytes()[8], b'_');
    assert_eq!(suffix.len(), 32);
}

#[test]
fn validate_session_id_rejects_slash() {
    let err = validate_session_id("foo/bar").unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn validate_session_id_rejects_backslash() {
    let err = validate_session_id("foo\\bar").unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn validate_session_id_rejects_dotdot() {
    let err = validate_session_id("..secret").unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn validate_session_id_rejects_colon() {
    let err = validate_session_id("C:drive").unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn validate_session_id_rejects_control_chars() {
    let err = validate_session_id("foo\nbar").unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn validate_session_id_rejects_empty() {
    let err = validate_session_id("").unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn validate_session_id_accepts_normal() {
    validate_session_id("20250315_120000").unwrap();
    validate_session_id("my-session").unwrap();
    validate_session_id("session_123").unwrap();
}

#[test]
fn save_load_roundtrip_with_custom_messages() {
    use swink_agent::{AgentMessage, CustomMessage, CustomMessageRegistry};

    #[derive(Debug)]
    struct TestCustomMsg {
        data: String,
    }

    impl CustomMessage for TestCustomMsg {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn type_name(&self) -> Option<&str> {
            Some("TestCustomMsg")
        }
        fn to_json(&self) -> Option<serde_json::Value> {
            Some(serde_json::json!({ "data": self.data }))
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let meta = SessionMeta {
        id: "test-full".to_string(),
        title: "Full roundtrip".to_string(),
        created_at: chrono::DateTime::from_timestamp(1_710_500_000, 0)
            .unwrap()
            .to_utc(),
        updated_at: chrono::DateTime::from_timestamp(1_710_500_000, 0)
            .unwrap()
            .to_utc(),
        version: 1,
        sequence: 0,
    };

    let messages: Vec<AgentMessage> = vec![
        AgentMessage::Llm(swink_agent::LlmMessage::User(
            swink_agent::UserMessage::new(vec![swink_agent::ContentBlock::Text {
                text: "Hello".to_string(),
            }])
            .with_timestamp(100),
        )),
        AgentMessage::Custom(Box::new(TestCustomMsg {
            data: "custom-payload".to_string(),
        })),
        AgentMessage::Llm(swink_agent::LlmMessage::User(
            swink_agent::UserMessage::new(vec![swink_agent::ContentBlock::Text {
                text: "World".to_string(),
            }])
            .with_timestamp(200),
        )),
    ];

    store.save("test-full", &meta, &messages).unwrap();

    // Load with registry — custom message restored
    let mut registry = CustomMessageRegistry::new();
    registry.register(
        "TestCustomMsg",
        Box::new(|val: serde_json::Value| {
            let data = val
                .get("data")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing data".to_string())?;
            Ok(Box::new(TestCustomMsg {
                data: data.to_string(),
            }) as Box<dyn CustomMessage>)
        }),
    );

    let (loaded_meta, loaded_messages) = store.load("test-full", Some(&registry)).unwrap();
    assert_eq!(loaded_meta.id, "test-full");
    assert_eq!(loaded_messages.len(), 3);
    assert!(matches!(
        loaded_messages[0],
        AgentMessage::Llm(swink_agent::LlmMessage::User(_))
    ));
    assert!(matches!(loaded_messages[1], AgentMessage::Custom(_)));
    assert!(matches!(
        loaded_messages[2],
        AgentMessage::Llm(swink_agent::LlmMessage::User(_))
    ));

    // Verify custom message content via downcast
    let custom = loaded_messages[1].downcast_ref::<TestCustomMsg>().unwrap();
    assert_eq!(custom.data, "custom-payload");

    // Load without registry — custom messages skipped
    let (_, loaded_no_reg) = store.load("test-full", None).unwrap();
    assert_eq!(loaded_no_reg.len(), 2);
    assert!(matches!(loaded_no_reg[0], AgentMessage::Llm(_)));
    assert!(matches!(loaded_no_reg[1], AgentMessage::Llm(_)));
}

#[test]
fn append_preserves_saved_state() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let now = now_utc();
    let meta = SessionMeta {
        id: "test-state".to_string(),
        title: "State".to_string(),
        created_at: now,
        updated_at: now,
        version: 1,
        sequence: 0,
    };

    let initial_messages: Vec<AgentMessage> = vec![AgentMessage::Llm(LlmMessage::User(
        swink_agent::UserMessage::new(vec![swink_agent::ContentBlock::Text {
            text: "hello".to_string(),
        }])
        .with_timestamp(1),
    ))];
    store.save("test-state", &meta, &initial_messages).unwrap();
    store
        .save_state("test-state", &serde_json::json!({ "cursor": 1 }))
        .unwrap();

    let appended_messages: Vec<AgentMessage> = vec![AgentMessage::Llm(LlmMessage::User(
        swink_agent::UserMessage::new(vec![swink_agent::ContentBlock::Text {
            text: "world".to_string(),
        }])
        .with_timestamp(2),
    ))];
    store.append("test-state", &appended_messages).unwrap();

    let state = store.load_state("test-state").unwrap();
    assert_eq!(state, Some(serde_json::json!({ "cursor": 1 })));

    let (_, messages) = store.load("test-state", None).unwrap();
    assert_eq!(messages.len(), 2);
}

#[test]
fn load_state_errors_on_corrupted_state_line() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let meta = fresh_meta("corrupt-state");
    store
        .save("corrupt-state", &meta, &[user_msg("hello", 1)])
        .unwrap();

    let path = session_path(dir.path(), "corrupt-state");
    let mut contents = std::fs::read_to_string(&path).unwrap();
    contents.push_str("{\"_state\":true,\"data\":\n");
    std::fs::write(&path, contents).unwrap();

    let err = store.load_state("corrupt-state").unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("state line"));
}

#[test]
fn atomic_write_leaves_no_temp_file_on_success() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("sess.jsonl");
    atomic_write(&target, |w| {
        w.write_all(b"hello\n")?;
        Ok(())
    })
    .unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello\n");
    // No leftover temp files in the directory
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        assert!(
            !name.contains(".tmp."),
            "unexpected temp file left behind: {name}"
        );
    }
}

#[test]
fn atomic_write_cleans_up_temp_on_error() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("sess.jsonl");
    let err = atomic_write(&target, |_w| {
        Err(io::Error::other("simulated mid-write failure"))
    })
    .unwrap_err();
    assert_eq!(err.to_string(), "simulated mid-write failure");
    // Target must not exist (no zero-length file)
    assert!(!target.exists());
    // And no leftover temp file
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        assert!(!name.contains(".tmp."), "temp file not cleaned up: {name}");
    }
}

#[test]
fn atomic_write_replaces_existing_file() {
    // Regression: on Windows, std::fs::rename does not replace an existing
    // destination, so rewrites of existing session files would fail.
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("sess.jsonl");
    std::fs::write(&target, b"old content\n").unwrap();
    atomic_write(&target, |w| {
        w.write_all(b"new content\n")?;
        Ok(())
    })
    .unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new content\n");
    // No leftover temp files
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        assert!(!name.contains(".tmp."), "temp file left behind: {name}");
    }
}

#[test]
fn atomic_write_concurrent_rewrites_of_same_target_do_not_collide() {
    // Regression: the temp file path must be unique per write attempt.
    // If two overlapping rewrites in the same process shared a temp path,
    // they could truncate or rename each other's files nondeterministically.
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::thread;

    let dir = tempfile::tempdir().unwrap();
    let target = Arc::new(dir.path().join("sess.jsonl"));
    std::fs::write(&*target, b"initial\n").unwrap();

    let n = 8;
    let barrier = Arc::new(Barrier::new(n));
    let mut handles = Vec::with_capacity(n);
    for i in 0..n {
        let target = Arc::clone(&target);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            atomic_write(&target, |w| {
                // Write enough data that a torn rename would be observable.
                for _ in 0..256 {
                    writeln!(w, "writer-{i}")?;
                }
                Ok(())
            })
        }));
    }
    for h in handles {
        h.join().unwrap().expect("concurrent atomic_write failed");
    }

    // Final file must be entirely one writer's content (no interleaving).
    let final_contents = std::fs::read_to_string(&*target).unwrap();
    let first_line = final_contents.lines().next().unwrap();
    assert!(first_line.starts_with("writer-"));
    for line in final_contents.lines() {
        assert_eq!(line, first_line, "file contains interleaved writes");
    }

    // No leftover temp files.
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        assert!(!name.contains(".tmp."), "temp file left behind: {name}");
    }
}

#[test]
fn save_preserves_previous_file_when_new_write_fails() {
    // Regression for #234: a failed rewrite must not truncate the live
    // file. With atomic rename, the original content survives.
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
    let now = now_utc();
    let meta = SessionMeta {
        id: "atomic".to_string(),
        title: "t".to_string(),
        created_at: now,
        updated_at: now,
        version: 1,
        sequence: 0,
    };
    let messages: Vec<AgentMessage> = vec![AgentMessage::Llm(LlmMessage::User(
        swink_agent::UserMessage::new(vec![swink_agent::ContentBlock::Text {
            text: "first".to_string(),
        }])
        .with_timestamp(1),
    ))];
    store.save("atomic", &meta, &messages).unwrap();

    let path = session_path(dir.path(), "atomic");
    let before = std::fs::read_to_string(&path).unwrap();
    assert!(!before.is_empty());

    // Simulate a failed rewrite. With atomic_write, the target file must
    // remain untouched.
    let _ = atomic_write(&path, |_w| Err(io::Error::other("boom")));

    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(before, after, "failed write must not corrupt live file");

    // File still parses cleanly
    let (_, loaded) = store.load("atomic", None).unwrap();
    assert_eq!(loaded.len(), 1);
}

#[test]
fn load_reads_message_entries_saved_via_save_entries() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let now = now_utc();
    let meta = SessionMeta {
        id: "entry-messages".to_string(),
        title: "Entry messages".to_string(),
        created_at: now,
        updated_at: now,
        version: 1,
        sequence: 0,
    };

    let entries = vec![
        SessionEntry::Message(LlmMessage::User(
            swink_agent::UserMessage::new(vec![swink_agent::ContentBlock::Text {
                text: "hello".to_string(),
            }])
            .with_timestamp(1),
        )),
        SessionEntry::Label {
            text: "bookmark".to_string(),
            message_index: 0,
            timestamp: 2,
        },
        SessionEntry::Message(LlmMessage::User(
            swink_agent::UserMessage::new(vec![swink_agent::ContentBlock::Text {
                text: "world".to_string(),
            }])
            .with_timestamp(3),
        )),
    ];

    store
        .save_entries("entry-messages", &meta, &entries)
        .unwrap();

    let (_, messages) = store.load("entry-messages", None).unwrap();
    assert_eq!(messages.len(), 2);
    assert!(matches!(
        messages[0],
        AgentMessage::Llm(LlmMessage::User(_))
    ));
    assert!(matches!(
        messages[1],
        AgentMessage::Llm(LlmMessage::User(_))
    ));
}

#[test]
fn save_preserves_state_and_rich_entries() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let meta = fresh_meta("preserve-save");
    let entries = vec![
        SessionEntry::Message(LlmMessage::User(
            swink_agent::UserMessage::new(vec![swink_agent::ContentBlock::Text {
                text: "hello".to_string(),
            }])
            .with_timestamp(1),
        )),
        SessionEntry::Label {
            text: "bookmark".to_string(),
            message_index: 0,
            timestamp: 2,
        },
    ];

    store
        .save_entries("preserve-save", &meta, &entries)
        .unwrap();
    store
        .save_state("preserve-save", &serde_json::json!({ "cursor": 7 }))
        .unwrap();

    let (loaded_meta, _) = store.load("preserve-save", None).unwrap();
    store
        .save(
            "preserve-save",
            &loaded_meta,
            &[user_msg("updated", 3), user_msg("again", 4)],
        )
        .unwrap();

    assert_eq!(
        store.load_state("preserve-save").unwrap(),
        Some(serde_json::json!({ "cursor": 7 }))
    );

    let (_, entries) = store.load_entries("preserve-save").unwrap();
    assert_eq!(entries.len(), 3, "messages plus preserved label");
    assert!(matches!(entries[0], SessionEntry::Message(_)));
    assert!(matches!(entries[1], SessionEntry::Message(_)));
    assert!(matches!(entries[2], SessionEntry::Label { .. }));
}

#[test]
fn save_entries_preserve_saved_state() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let meta = fresh_meta("preserve-entry-save");
    store
        .save("preserve-entry-save", &meta, &[user_msg("hello", 1)])
        .unwrap();
    store
        .save_state("preserve-entry-save", &serde_json::json!({ "cursor": 11 }))
        .unwrap();

    let (loaded_meta, _) = store.load("preserve-entry-save", None).unwrap();
    let entries = vec![
        SessionEntry::Message(LlmMessage::User(
            swink_agent::UserMessage::new(vec![swink_agent::ContentBlock::Text {
                text: "updated".to_string(),
            }])
            .with_timestamp(2),
        )),
        SessionEntry::Label {
            text: "kept".to_string(),
            message_index: 0,
            timestamp: 3,
        },
    ];

    store
        .save_entries("preserve-entry-save", &loaded_meta, &entries)
        .unwrap();

    assert_eq!(
        store.load_state("preserve-entry-save").unwrap(),
        Some(serde_json::json!({ "cursor": 11 }))
    );
}

#[test]
fn save_entries_preserves_existing_custom_message_envelopes() {
    use swink_agent::{CustomMessage, CustomMessageRegistry};

    #[derive(Debug)]
    struct TestCustomMsg {
        data: String,
    }

    impl CustomMessage for TestCustomMsg {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn type_name(&self) -> Option<&str> {
            Some("TestCustomMsg")
        }

        fn to_json(&self) -> Option<serde_json::Value> {
            Some(serde_json::json!({ "data": self.data }))
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let meta = fresh_meta("preserve-entry-custom");
    let messages = vec![
        user_msg("hello", 1),
        AgentMessage::Custom(Box::new(TestCustomMsg {
            data: "custom-payload".to_string(),
        })),
    ];
    store
        .save("preserve-entry-custom", &meta, &messages)
        .unwrap();

    let (loaded_meta, _) = store.load("preserve-entry-custom", None).unwrap();
    let entries = vec![
        SessionEntry::Message(LlmMessage::User(
            swink_agent::UserMessage::new(vec![swink_agent::ContentBlock::Text {
                text: "updated".to_string(),
            }])
            .with_timestamp(2),
        )),
        SessionEntry::Label {
            text: "kept".to_string(),
            message_index: 0,
            timestamp: 3,
        },
    ];

    store
        .save_entries("preserve-entry-custom", &loaded_meta, &entries)
        .unwrap();

    let mut registry = CustomMessageRegistry::new();
    registry.register(
        "TestCustomMsg",
        Box::new(|val: serde_json::Value| {
            let data = val
                .get("data")
                .and_then(|value| value.as_str())
                .ok_or_else(|| "missing data".to_string())?;
            Ok(Box::new(TestCustomMsg {
                data: data.to_string(),
            }) as Box<dyn CustomMessage>)
        }),
    );

    let (_, loaded_messages) = store
        .load("preserve-entry-custom", Some(&registry))
        .unwrap();
    assert_eq!(loaded_messages.len(), 2, "message plus preserved custom");
    assert!(matches!(loaded_messages[0], AgentMessage::Llm(_)));
    let custom = loaded_messages[1].downcast_ref::<TestCustomMsg>().unwrap();
    assert_eq!(custom.data, "custom-payload");
}

fn user_msg(text: &str, ts: u64) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(
        swink_agent::UserMessage::new(vec![swink_agent::ContentBlock::Text {
            text: text.to_string(),
        }])
        .with_timestamp(ts),
    ))
}

fn fresh_meta(id: &str) -> SessionMeta {
    let now = now_utc();
    SessionMeta {
        id: id.to_string(),
        title: "t".to_string(),
        created_at: now,
        updated_at: now,
        version: 1,
        sequence: 0,
    }
}

fn user_entry(text: &str, ts: u64) -> SessionEntry {
    SessionEntry::Message(LlmMessage::User(
        swink_agent::UserMessage::new(vec![swink_agent::ContentBlock::Text {
            text: text.to_string(),
        }])
        .with_timestamp(ts),
    ))
}

#[test]
fn append_entries_appends_without_rewriting_existing_lines() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let meta = fresh_meta("append-entries");
    store
        .save_entries("append-entries", &meta, &[user_entry("first", 1)])
        .unwrap();
    let (created_meta, _) = store.load_entries("append-entries").unwrap();

    // Capture the byte length of the existing records so we can prove the
    // append did not rewrite them.
    let path = session_path(dir.path(), "append-entries");
    let before = std::fs::read_to_string(&path).unwrap();

    let returned = store
        .append_entries(
            "append-entries",
            &created_meta,
            &[user_entry("second", 2), user_entry("third", 3)],
        )
        .unwrap();

    // Sequence bumped and returned without a re-read.
    assert_eq!(returned.sequence, created_meta.sequence + 1);

    // All entries are present and ordered.
    let (loaded_meta, entries) = store.load_entries("append-entries").unwrap();
    assert_eq!(loaded_meta.sequence, returned.sequence);
    let texts: Vec<&str> = entries
        .iter()
        .filter_map(|e| match e {
            SessionEntry::Message(LlmMessage::User(u)) => match &u.content[0] {
                swink_agent::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["first", "second", "third"]);

    // The original records remain verbatim at the head of the file —
    // i.e. the append did not rewrite them.
    let after = std::fs::read_to_string(&path).unwrap();
    let first_record_line = before.lines().nth(1).unwrap();
    assert!(
        after.contains(first_record_line),
        "first record should be preserved byte-for-byte by an in-place append"
    );
    assert!(
        after.len() > before.len(),
        "appended records should grow the file"
    );
}

#[test]
fn append_entries_rejects_sequence_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let meta = fresh_meta("append-conflict");
    store
        .save_entries("append-conflict", &meta, &[user_entry("first", 1)])
        .unwrap();

    // Stale meta (sequence behind the on-disk value) must be rejected.
    let mut stale = meta;
    stale.sequence = 99;
    let err = store
        .append_entries("append-conflict", &stale, &[user_entry("second", 2)])
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
}

#[test]
fn append_entries_errors_on_missing_session() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let meta = fresh_meta("nope");
    let err = store
        .append_entries("nope", &meta, &[user_entry("x", 1)])
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
}

#[test]
fn search_scans_across_saved_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let mut meta_a = fresh_meta("search-a");
    meta_a.title = "Auth notes".to_string();
    store
        .save(
            "search-a",
            &meta_a,
            &[
                user_msg("We decided the auth middleware owns refresh tokens", 10),
                user_msg("Unrelated deployment note", 11),
            ],
        )
        .unwrap();

    let mut meta_b = fresh_meta("search-b");
    meta_b.title = "Billing notes".to_string();
    store
        .save(
            "search-b",
            &meta_b,
            &[user_msg("Billing retries use exponential backoff", 20)],
        )
        .unwrap();

    let hits = store
        .search("auth middleware", &SessionSearchOptions::default())
        .unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].session_id, "search-a");
    assert_eq!(hits[0].session_title, "Auth notes");
    assert!(hits[0].snippet.contains("auth middleware"));
    assert!(matches!(hits[0].entry, SessionEntry::Message(_)));
}

#[test]
fn save_canonicalizes_mismatched_metadata_id() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    store
        .save(
            "canonical-save",
            &fresh_meta("wrong-save"),
            &[user_msg("canonical metadata", 10)],
        )
        .unwrap();

    let session_file = dir.path().join("canonical-save.jsonl");
    let first_line = std::fs::read_to_string(session_file)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();
    let raw_meta: SessionMeta = serde_json::from_str(&first_line).unwrap();
    assert_eq!(raw_meta.id, "canonical-save");

    let (loaded_meta, _) = store.load("canonical-save", None).unwrap();
    assert_eq!(loaded_meta.id, "canonical-save");

    let listed = store.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "canonical-save");
}

#[test]
fn save_full_canonicalizes_returned_and_persisted_metadata_id() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let returned = store
        .save_full(
            "canonical-full",
            &fresh_meta("wrong-full"),
            &[user_msg("full canonical metadata", 10)],
            &serde_json::json!({"persisted": true}),
        )
        .unwrap();

    assert_eq!(returned.id, "canonical-full");
    let (loaded_meta, _, loaded_state) = store.load_full("canonical-full", None).unwrap();
    assert_eq!(loaded_meta.id, "canonical-full");
    assert_eq!(loaded_state, Some(serde_json::json!({"persisted": true})));
}

#[test]
fn search_uses_canonical_session_id_for_mismatched_saved_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    store
        .save_entries(
            "canonical-search",
            &fresh_meta("wrong-search"),
            &[user_entry("canonical search metadata", 10)],
        )
        .unwrap();

    let hits = store
        .search("canonical metadata", &SessionSearchOptions::default())
        .unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].session_id, "canonical-search");
}

#[test]
fn search_respects_session_type_time_and_limit_filters() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    store
        .save_entries(
            "filtered-a",
            &fresh_meta("filtered-a"),
            &[
                user_entry("auth middleware message", 10),
                SessionEntry::Label {
                    text: "auth middleware bookmark".to_string(),
                    message_index: 0,
                    timestamp: 20,
                },
                SessionEntry::Label {
                    text: "auth middleware late bookmark".to_string(),
                    message_index: 1,
                    timestamp: 40,
                },
            ],
        )
        .unwrap();
    store
        .save_entries(
            "filtered-b",
            &fresh_meta("filtered-b"),
            &[SessionEntry::Label {
                text: "auth middleware other session".to_string(),
                message_index: 0,
                timestamp: 20,
            }],
        )
        .unwrap();

    let options = SessionSearchOptions {
        session_ids: Some(vec!["filtered-a".to_string()]),
        entry_types: Some(vec!["label".to_string()]),
        start_time: Some(chrono::DateTime::from_timestamp(15, 0).unwrap().to_utc()),
        end_time: Some(chrono::DateTime::from_timestamp(25, 0).unwrap().to_utc()),
        max_results: Some(1),
    };

    let hits = store.search("auth middleware", &options).unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].session_id, "filtered-a");
    assert!(matches!(
        hits[0].entry,
        SessionEntry::Label { timestamp: 20, .. }
    ));
}

fn rewrite_meta_without_padding(path: &Path, id: &str, update: impl FnOnce(&mut SessionMeta)) {
    let (mut meta, lines) = read_meta_and_message_lines(path, id).unwrap();
    update(&mut meta);

    let mut contents = format!("{}\n", serde_json::to_string(&meta).unwrap());
    for line in lines {
        contents.push_str(&line);
        contents.push('\n');
    }
    std::fs::write(path, contents).unwrap();
}

#[test]
fn append_advances_sequence_and_rejects_stale_save() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let meta = fresh_meta("seq-append");
    store
        .save("seq-append", &meta, &[user_msg("a", 1)])
        .unwrap();
    // After save, on-disk sequence == 1. `meta` still holds 0.

    store.append("seq-append", &[user_msg("b", 2)]).unwrap();
    // After append, on-disk sequence must have advanced to 2.

    // Sanity: list() / load() sees the bumped sequence.
    let (loaded_meta, _) = store.load("seq-append", None).unwrap();
    assert_eq!(loaded_meta.sequence, 2);

    // A stale writer holding the pre-append meta (sequence == 1 after save)
    // should now be rejected by check_sequence.
    let mut stale = meta;
    stale.sequence = 1;
    let err = store
        .save("seq-append", &stale, &[user_msg("c", 3)])
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
}

#[test]
fn append_extends_file_without_rewriting_existing_records() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let meta = fresh_meta("append-in-place");
    store
        .save("append-in-place", &meta, &[user_msg("first", 1)])
        .unwrap();

    let path = session_path(dir.path(), "append-in-place");
    let before = std::fs::read_to_string(&path).unwrap();
    let before_lines = before.lines().collect::<Vec<_>>();
    let before_meta_line_len = before_lines[0].len();
    let before_message_line = before_lines[1].to_string();

    store
        .append("append-in-place", &[user_msg("second", 2)])
        .unwrap();

    let after = std::fs::read_to_string(&path).unwrap();
    let after_lines = after.lines().collect::<Vec<_>>();
    assert_eq!(
        after_lines[0].len(),
        before_meta_line_len,
        "append should patch the reserved metadata line in place"
    );
    assert_eq!(
        after_lines[1], before_message_line,
        "append must leave existing record bytes untouched"
    );
    assert_eq!(
        after_lines.len(),
        5,
        "append should add begin, record, and internal metadata commit lines"
    );

    let (loaded_meta, loaded_messages) = store.load("append-in-place", None).unwrap();
    assert_eq!(loaded_meta.sequence, 2);
    assert_eq!(loaded_messages.len(), 2);
}

#[test]
fn append_failure_before_metadata_commit_discards_uncommitted_records() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let meta = fresh_meta("append-before-commit");
    store
        .save("append-before-commit", &meta, &[user_msg("first", 1)])
        .unwrap();

    let path = session_path(dir.path(), "append-before-commit");
    let (mut append_meta, meta_line_len) =
        read_meta_with_line_len(&path, "append-before-commit").unwrap();
    append_meta.updated_at = now_utc();
    append_meta.sequence += 1;
    let second_line = SessionRecord::from_message(&user_msg("second", 2), "append-before-commit")
        .unwrap()
        .to_json_line()
        .unwrap();

    let err = append_records_in_place_with_hooks(
        &path,
        &append_meta,
        meta_line_len,
        &[second_line],
        |_| Err(io::Error::other("simulated commit write failure")),
        |_| Ok(()),
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "simulated commit write failure");

    let (loaded_meta, loaded_messages) = store.load("append-before-commit", None).unwrap();
    assert_eq!(
        loaded_meta.sequence, 1,
        "metadata must remain at the prior committed sequence"
    );
    assert_eq!(
        loaded_messages.len(),
        1,
        "record lines after an unclosed append-begin marker are uncommitted"
    );
}

#[test]
fn append_failure_before_metadata_cache_patch_recovers_committed_records() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let meta = fresh_meta("append-commit-first");
    store
        .save("append-commit-first", &meta, &[user_msg("first", 1)])
        .unwrap();

    let path = session_path(dir.path(), "append-commit-first");
    let (mut append_meta, meta_line_len) =
        read_meta_with_line_len(&path, "append-commit-first").unwrap();
    append_meta.updated_at = now_utc();
    append_meta.sequence += 1;
    let second_line = SessionRecord::from_message(&user_msg("second", 2), "append-commit-first")
        .unwrap()
        .to_json_line()
        .unwrap();

    let err = append_records_in_place_with_hook(
        &path,
        &append_meta,
        meta_line_len,
        &[second_line],
        |_| Err(io::Error::other("simulated cache patch failure")),
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "simulated cache patch failure");

    let first_line_meta: SessionMeta = serde_json::from_str(
        std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        first_line_meta.sequence, 1,
        "line 1 remains the old cache if the metadata patch is interrupted"
    );

    let (loaded_meta, loaded_messages) = store.load("append-commit-first", None).unwrap();
    assert_eq!(
        loaded_meta.sequence, 2,
        "load must recover the committed metadata record after an interrupted cache patch"
    );
    let listed = store.list().unwrap();
    assert_eq!(
        listed[0].sequence, 2,
        "list must recover the committed metadata record after an interrupted cache patch"
    );
    assert_eq!(
        loaded_messages.len(),
        2,
        "committed append records must remain recoverable after cache patch failure"
    );

    let mut stale = meta;
    stale.sequence = 1;
    let err = store
        .save("append-commit-first", &stale, &[user_msg("stale", 3)])
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
}

fn meta_commit_line(meta: &SessionMeta) -> String {
    SessionRecord::Meta(Box::new(meta.clone()))
        .to_json_line()
        .unwrap()
}

fn message_line(text: &str, ts: u64, id: &str) -> String {
    SessionRecord::from_message(&user_msg(text, ts), id)
        .unwrap()
        .to_json_line()
        .unwrap()
}

/// Writes a session file with a padded first metadata line followed by
/// `body` verbatim (the caller controls trailing newlines / torn tails).
fn write_raw_session_file(path: &Path, first_meta: &SessionMeta, body: &[u8]) {
    let mut file = std::fs::File::create(path).unwrap();
    write_meta_line(&mut file, first_meta, META_LINE_PADDING).unwrap();
    file.write_all(body).unwrap();
}

/// A ~1 KiB record line that is not a Meta commit.
fn pad_line() -> String {
    format!("{{\"pad\":\"{}\"}}", "x".repeat(1024))
}

/// Asserts `read_meta_with_line_len` returns exactly what the forward
/// line-by-line scan (`read_meta_and_message_lines`) computes, then
/// returns the meta for further assertions.
fn read_meta_checked_against_forward_scan(path: &Path, id: &str) -> SessionMeta {
    let (meta, _) = read_meta_with_line_len(path, id).unwrap();
    let (oracle, _) = read_meta_and_message_lines(path, id).unwrap();
    assert_eq!(
        meta, oracle,
        "tail scan must return exactly what the forward scan returns"
    );
    meta
}

#[test]
fn read_meta_falls_back_to_first_line_without_meta_commits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("no-commits.jsonl");
    let first = fresh_meta("no-commits");
    let body = format!(
        "{}\n{}\n",
        message_line("first", 1, "no-commits"),
        message_line("second", 2, "no-commits"),
    );
    write_raw_session_file(&path, &first, body.as_bytes());

    let meta = read_meta_checked_against_forward_scan(&path, "no-commits");
    assert_eq!(meta, first);

    let raw = std::fs::read_to_string(&path).unwrap();
    let (_, line_len) = read_meta_with_line_len(&path, "no-commits").unwrap();
    assert_eq!(
        line_len,
        raw.find('\n').unwrap() + 1,
        "line_len must be the padded first line including its newline"
    );
}

#[test]
fn read_meta_returns_last_meta_commit_like_the_forward_scan() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("multi-meta.jsonl");
    let mut first = fresh_meta("multi-meta");
    first.title = "stale".to_string();

    let mut second = first.clone();
    second.title = "second".to_string();
    second.sequence = 2;
    // Commit records deliberately carry a different id: the reader must
    // canonicalize to the file id exactly like the forward scan did.
    second.id = "not-the-file-id".to_string();
    let mut third = second.clone();
    third.title = "third".to_string();
    third.sequence = 3;

    let body = format!(
        "{}\n{}\n{}\n{}\n",
        message_line("m1", 1, "multi-meta"),
        meta_commit_line(&second),
        message_line("m2", 2, "multi-meta"),
        meta_commit_line(&third),
    );
    write_raw_session_file(&path, &first, body.as_bytes());

    let meta = read_meta_checked_against_forward_scan(&path, "multi-meta");
    assert_eq!(meta.title, "third", "the LAST Meta commit record wins");
    assert_eq!(meta.sequence, 3);
    assert_eq!(
        meta.id, "multi-meta",
        "meta id must be canonicalized to the file id"
    );
}

#[test]
fn read_meta_skips_trailing_partial_line_after_commit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("torn-tail.jsonl");
    let first = fresh_meta("torn-tail");
    let mut committed = first.clone();
    committed.title = "committed".to_string();
    committed.sequence = 2;

    let mut body = format!(
        "{}\n{}\n",
        message_line("m1", 1, "torn-tail"),
        meta_commit_line(&committed),
    );
    // Crash-torn tail: a truncated record with no trailing newline. It is
    // valid UTF-8 but unparseable JSON, so it must be skipped.
    body.push_str("{\"_meta\":true,\"data\":{\"id\":\"torn");
    write_raw_session_file(&path, &first, body.as_bytes());

    let meta = read_meta_checked_against_forward_scan(&path, "torn-tail");
    assert_eq!(meta.title, "committed");
    assert_eq!(meta.sequence, 2);
}

#[test]
fn read_meta_errors_on_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.jsonl");
    std::fs::File::create(&path).unwrap();

    let err = read_meta_with_line_len(&path, "empty").unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("empty session file"));
}

#[test]
fn read_meta_finds_last_commit_within_tail_window_of_large_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large-tail-commit.jsonl");
    let first = fresh_meta("large-tail-commit");
    let mut committed = first.clone();
    committed.title = "committed".to_string();
    committed.sequence = 2;

    let mut body = String::new();
    for _ in 0..80 {
        body.push_str(&pad_line());
        body.push('\n');
    }
    body.push_str(&meta_commit_line(&committed));
    body.push('\n');
    assert!(
        body.len() as u64 > META_TAIL_WINDOW,
        "body must exceed the initial tail window to exercise the windowed scan"
    );
    write_raw_session_file(&path, &first, body.as_bytes());

    let meta = read_meta_checked_against_forward_scan(&path, "large-tail-commit");
    assert_eq!(meta.title, "committed");
    assert_eq!(meta.sequence, 2);
}

#[test]
fn read_meta_recovers_commit_buried_before_large_uncommitted_tail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("buried-commit.jsonl");
    let first = fresh_meta("buried-commit");
    let mut committed = first.clone();
    committed.title = "committed".to_string();
    committed.sequence = 2;

    // The only Meta commit sits before a crashed (uncommitted) append
    // whose records span several tail windows, forcing the scan to widen
    // and ultimately fall back to the full forward pass.
    let mut body = format!(
        "{}\n{}\n{}\n",
        message_line("m1", 1, "buried-commit"),
        meta_commit_line(&committed),
        SessionRecord::AppendBegin.to_json_line().unwrap(),
    );
    for _ in 0..200 {
        body.push_str(&pad_line());
        body.push('\n');
    }
    assert!(body.len() as u64 > 2 * META_TAIL_WINDOW);
    write_raw_session_file(&path, &first, body.as_bytes());

    let meta = read_meta_checked_against_forward_scan(&path, "buried-commit");
    assert_eq!(meta.title, "committed");
    assert_eq!(meta.sequence, 2);
}

#[test]
fn read_meta_first_line_wins_in_large_file_without_commits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large-no-commits.jsonl");
    let first = fresh_meta("large-no-commits");

    let mut body = String::new();
    for _ in 0..200 {
        body.push_str(&pad_line());
        body.push('\n');
    }
    assert!(body.len() as u64 > 2 * META_TAIL_WINDOW);
    write_raw_session_file(&path, &first, body.as_bytes());

    let meta = read_meta_checked_against_forward_scan(&path, "large-no-commits");
    assert_eq!(meta, first);
}

#[test]
fn read_meta_tolerates_torn_multibyte_tail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("torn-utf8.jsonl");
    let first = fresh_meta("torn-utf8");
    let mut committed = first.clone();
    committed.title = "committed".to_string();
    committed.sequence = 2;

    // A crash mid-write can truncate a multi-byte UTF-8 character in the
    // tail line. Semantics per spec 021 FR-004 addendum (#1067): the torn
    // line is skipped and the last committed meta is still recovered,
    // in both the tail scan and the forward scan.
    let mut body = format!("{}\n", meta_commit_line(&committed)).into_bytes();
    body.extend_from_slice(b"{\"_meta\":true,\xE2\x82");
    write_raw_session_file(&path, &first, &body);

    let meta = read_meta_checked_against_forward_scan(&path, "torn-utf8");
    assert_eq!(meta, committed);
}

#[test]
fn read_meta_tolerates_torn_multibyte_tail_in_large_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large-torn-utf8.jsonl");
    let first = fresh_meta("large-torn-utf8");
    let mut committed = first.clone();
    committed.title = "committed".to_string();
    committed.sequence = 2;

    let mut body = String::new();
    for _ in 0..80 {
        body.push_str(&pad_line());
        body.push('\n');
    }
    body.push_str(&meta_commit_line(&committed));
    body.push('\n');
    assert!(body.len() as u64 > META_TAIL_WINDOW);
    let mut body = body.into_bytes();
    body.extend_from_slice(b"{\"_meta\":true,\xE2\x82");
    write_raw_session_file(&path, &first, &body);

    let meta = read_meta_checked_against_forward_scan(&path, "large-torn-utf8");
    assert_eq!(
        meta, committed,
        "windowed tail scan must skip invalid UTF-8 like the forward scan"
    );
}

#[test]
fn list_reports_last_committed_meta_despite_trailing_garbage() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let mut first = fresh_meta("listed-session");
    first.title = "stale".to_string();
    let mut second = first.clone();
    second.title = "second".to_string();
    second.sequence = 2;
    let mut third = first.clone();
    third.title = "third".to_string();
    third.sequence = 3;

    let mut body = format!(
        "{}\n{}\n{}\n{}\n",
        message_line("m1", 1, "listed-session"),
        meta_commit_line(&second),
        message_line("m2", 2, "listed-session"),
        meta_commit_line(&third),
    );
    body.push_str("{\"_meta\":true,\"data\":{\"id\":\"torn");
    let path = session_path(dir.path(), "listed-session");
    write_raw_session_file(&path, &first, body.as_bytes());

    let listed = store.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "listed-session");
    assert_eq!(
        listed[0].title, "third",
        "list must surface the last committed Meta record"
    );
    assert_eq!(listed[0].sequence, 3);
}

#[test]
fn load_waits_for_in_flight_append_commit_before_metadata_cache_patch() {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let meta = fresh_meta("append-load-race");
    store
        .save("append-load-race", &meta, &[user_msg("first", 1)])
        .unwrap();

    let path = session_path(dir.path(), "append-load-race");
    let (patched_tx, patched_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();

    let append_path = path;
    let append_handle = thread::spawn(move || {
        with_target_lock(&append_path, || {
            let (mut append_meta, meta_line_len) =
                read_meta_with_line_len(&append_path, "append-load-race")?;
            append_meta.updated_at = now_utc();
            append_meta.sequence += 1;
            let second_line =
                SessionRecord::from_message(&user_msg("second", 2), "append-load-race")
                    .unwrap()
                    .to_json_line()?;

            append_records_in_place_with_hook(
                &append_path,
                &append_meta,
                meta_line_len,
                &[second_line],
                |_| {
                    patched_tx.send(()).unwrap();
                    resume_rx.recv().unwrap();
                    Ok(())
                },
            )?;
            Ok(())
        })
    });

    patched_rx.recv().unwrap();

    let load_store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
    let (loaded_tx, loaded_rx) = mpsc::channel();
    let load_handle = thread::spawn(move || {
        loaded_tx
            .send(load_store.load("append-load-race", None))
            .unwrap();
    });

    assert!(
        loaded_rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "load must wait for an append that has committed records but not patched line 1"
    );

    resume_tx.send(()).unwrap();
    append_handle
        .join()
        .unwrap()
        .expect("append should finish cleanly");

    let (loaded_meta, loaded_messages) = loaded_rx.recv().unwrap().unwrap();
    load_handle.join().unwrap();
    assert_eq!(loaded_meta.sequence, 2);
    assert_eq!(
        loaded_messages.len(),
        2,
        "load must expose committed records and metadata together"
    );
}

#[test]
fn append_rewrite_failure_preserves_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut meta = fresh_meta("append-atomic");
    meta.sequence = 9;
    // Pin updated_at to a whole second so the metadata line on disk is the
    // shortest form chrono emits (SecondsFormat::AutoSi drops the fractional
    // part entirely when nanos == 0). append_records_with_rewrite refreshes
    // updated_at to now_utc(), whose fractional part is 0/3/6/9 digits wide,
    // so the refreshed line can only be the same width or wider — and
    // sequence 9 -> 10 adds one more character. That makes the line
    // guaranteed to outgrow meta_line_len and take the rewrite path.
    //
    // Without the pin this test was ~1-in-10 flaky: when now_utc() happened
    // to serialize shorter than the original timestamp, the new metadata
    // still fit in place, append_records_in_place returned true, and
    // rewrite_fn never ran.
    meta.updated_at = meta.updated_at.with_nanosecond(0).unwrap();
    let path = session_path(dir.path(), "append-atomic");
    let first_line = SessionRecord::from_message(&user_msg("first", 1), "append-atomic").unwrap();
    std::fs::write(
        &path,
        format!(
            "{}\n{}\n",
            serde_json::to_string(&meta).unwrap(),
            first_line.to_json_line().unwrap()
        ),
    )
    .unwrap();
    let before = std::fs::read_to_string(&path).unwrap();

    let err = append_records_with_rewrite(
        &path,
        "append-atomic",
        [SessionRecord::from_message(&user_msg("second", 2), "append-atomic").unwrap()],
        |_path, _meta, _lines| Err(io::Error::other("simulated append rewrite failure")),
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "simulated append rewrite failure");

    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        before, after,
        "failed append rewrite must not modify the live session file"
    );

    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
    let (loaded_meta, loaded_messages) = store.load("append-atomic", None).unwrap();
    assert_eq!(loaded_meta.sequence, 9);
    assert_eq!(loaded_messages.len(), 1);
}

#[test]
fn delete_waits_for_append_lock_and_does_not_allow_resurrection() {
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::thread;

    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(JsonlSessionStore::new(dir.path().to_path_buf()).unwrap());
    let meta = fresh_meta("delete-race");
    store
        .save("delete-race", &meta, &[user_msg("first", 1)])
        .unwrap();

    let path = session_path(dir.path(), "delete-race");
    rewrite_meta_without_padding(&path, "delete-race", |meta| {
        meta.sequence = 9;
        // Pin updated_at to a whole second matching append_rewrite_failure_preserves_existing_file
        // so sequence 9 -> 10 guarantees taking the rewrite path rather than in-place append.
        meta.updated_at = meta.updated_at.with_nanosecond(0).unwrap();
    });
    let append_ready = Arc::new(Barrier::new(2));
    let allow_rewrite = Arc::new(Barrier::new(2));

    let append_path = path.clone();
    let append_ready_for_thread = Arc::clone(&append_ready);
    let allow_rewrite_for_thread = Arc::clone(&allow_rewrite);
    let append_handle = thread::spawn(move || {
        append_records_with_rewrite(
            &append_path,
            "delete-race",
            [SessionRecord::from_message(&user_msg("second", 2), "delete-race").unwrap()],
            |path, meta, lines| {
                append_ready_for_thread.wait();
                allow_rewrite_for_thread.wait();
                rewrite_session_file_locked(path, meta, lines)
            },
        )
    });

    append_ready.wait();

    let delete_store = Arc::clone(&store);
    let delete_handle = thread::spawn(move || delete_store.delete("delete-race"));

    // If delete bypassed the session lock, it could return before the
    // append rewrites and the session would be recreated after delete.
    allow_rewrite.wait();

    append_handle
        .join()
        .unwrap()
        .expect("append should finish cleanly");
    delete_handle
        .join()
        .unwrap()
        .expect("delete should wait for the append lock");

    assert!(
        !path.exists(),
        "delete must not allow a concurrent append to resurrect the session"
    );
}

#[test]
fn save_state_advances_sequence_and_rejects_stale_save() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let meta = fresh_meta("seq-state");
    store.save("seq-state", &meta, &[user_msg("a", 1)]).unwrap();
    // On-disk sequence == 1.

    store
        .save_state("seq-state", &serde_json::json!({ "cursor": 1 }))
        .unwrap();

    let (loaded_meta, _) = store.load("seq-state", None).unwrap();
    assert_eq!(loaded_meta.sequence, 2);

    // Stale writer with sequence == 1 must be rejected.
    let mut stale = meta;
    stale.sequence = 1;
    let err = store
        .save("seq-state", &stale, &[user_msg("c", 3)])
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
}

#[test]
fn save_full_updates_messages_and_state_with_single_sequence_bump() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let meta = fresh_meta("save-full");
    store
        .save("save-full", &meta, &[user_msg("before", 1)])
        .unwrap();
    store
        .save_state("save-full", &serde_json::json!({ "cursor": 1 }))
        .unwrap();

    let (loaded_meta, _) = store.load("save-full", None).unwrap();
    assert_eq!(loaded_meta.sequence, 2);

    let persisted_meta = store
        .save_full(
            "save-full",
            &loaded_meta,
            &[user_msg("after", 2), user_msg("again", 3)],
            &serde_json::json!({ "cursor": 9, "draft": "synced" }),
        )
        .unwrap();

    assert_eq!(
        persisted_meta.sequence, 3,
        "combined transcript+state save should advance sequence once"
    );

    let (reloaded_meta, reloaded_messages) = store.load("save-full", None).unwrap();
    assert_eq!(reloaded_meta.sequence, 3);
    assert_eq!(reloaded_messages.len(), 2);
    assert_eq!(
        store.load_state("save-full").unwrap(),
        Some(serde_json::json!({ "cursor": 9, "draft": "synced" }))
    );
}

#[cfg(feature = "search")]
#[test]
fn save_full_updates_warm_search_index() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let meta = fresh_meta("save-full-search");
    store.open_search_index().unwrap();
    store
        .save(
            "save-full-search",
            &meta,
            &[user_msg("stale auth decision", 1)],
        )
        .unwrap();

    let (loaded_meta, _) = store.load("save-full-search", None).unwrap();
    store
        .save_full(
            "save-full-search",
            &loaded_meta,
            &[user_msg("fresh billing decision", 2)],
            &serde_json::json!({ "cursor": 2 }),
        )
        .unwrap();

    let fresh_hits = store
        .search("fresh billing", &SessionSearchOptions::default())
        .unwrap();
    assert_eq!(fresh_hits.len(), 1);
    assert_eq!(fresh_hits[0].session_id, "save-full-search");

    let stale_hits = store
        .search("stale auth", &SessionSearchOptions::default())
        .unwrap();
    assert!(
        stale_hits.is_empty(),
        "save_full should replace stale search documents"
    );
}

/// Append paths must extend a warm index incrementally — adding only the
/// new entries' documents — never by reloading and re-indexing the whole
/// session (the pre-fix behavior, which made every append O(session)).
///
/// Instrumented via `TantivyIndex::full_reindex_count`, a test-only
/// counter of whole-session (re)index operations: cheap, exact, and it
/// fails loudly if an append path regresses to `index_session`.
#[cfg(feature = "search")]
#[test]
fn append_paths_update_search_index_without_full_reindex() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
    store.open_search_index().unwrap();

    let meta = fresh_meta("append-incremental");
    store
        .save_entries("append-incremental", &meta, &[user_entry("quokka", 1)])
        .unwrap();
    let index = store.active_tantivy_index().expect("index is open");
    let after_save = index.full_reindex_count();

    let (loaded_meta, _) = store.load_entries("append-incremental").unwrap();
    let bumped = store
        .append_entries(
            "append-incremental",
            &loaded_meta,
            &[user_entry("wombat", 2)],
        )
        .unwrap();
    store
        .append_entries("append-incremental", &bumped, &[user_entry("numbat", 3)])
        .unwrap();
    store
        .append("append-incremental", &[user_msg("kookaburra", 4)])
        .unwrap();

    assert_eq!(
        index.full_reindex_count(),
        after_save,
        "append paths must not re-index the whole session"
    );

    // Everything — original and appended — is searchable. The first
    // search triggers the lazy full build (which legitimately calls
    // index_session), so it runs after the counter assertion above.
    for term in ["quokka", "wombat", "numbat", "kookaburra"] {
        let hits = store
            .search(term, &SessionSearchOptions::default())
            .unwrap();
        assert_eq!(hits.len(), 1, "expected exactly one hit for {term:?}");
        assert_eq!(hits[0].session_id, "append-incremental");
    }

    // Steady state: once the index is built, appends stay incremental
    // and are immediately searchable.
    let after_build = index.full_reindex_count();
    let (steady_meta, _) = store.load_entries("append-incremental").unwrap();
    store
        .append_entries(
            "append-incremental",
            &steady_meta,
            &[user_entry("capybara", 5)],
        )
        .unwrap();
    assert_eq!(
        index.full_reindex_count(),
        after_build,
        "post-build appends must stay incremental"
    );
    let hits = store
        .search("capybara", &SessionSearchOptions::default())
        .unwrap();
    assert_eq!(hits.len(), 1);
}

#[test]
fn load_full_returns_messages_and_state_from_one_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let persisted_meta = store
        .save_full(
            "load-full",
            &fresh_meta("load-full"),
            &[user_msg("hello", 1), user_msg("again", 2)],
            &serde_json::json!({ "cursor": 4, "draft": "stable" }),
        )
        .unwrap();

    let (loaded_meta, loaded_messages, loaded_state) = store.load_full("load-full", None).unwrap();

    assert_eq!(loaded_meta.sequence, persisted_meta.sequence);
    assert_eq!(loaded_messages.len(), 2);
    assert_eq!(
        loaded_state,
        Some(serde_json::json!({ "cursor": 4, "draft": "stable" }))
    );
}

#[test]
fn stale_saves_do_not_both_pass_validation() {
    use std::sync::mpsc;
    use std::thread;

    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let meta = fresh_meta("stale-race");
    store
        .save("stale-race", &meta, &[user_msg("v1", 1)])
        .unwrap();
    let (stale_meta, _) = store.load("stale-race", None).unwrap();

    let path = session_path(dir.path(), "stale-race");
    let (validated_tx, validated_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();

    let thread_path = path;
    let thread_meta = stale_meta.clone();
    let paused_writer = thread::spawn(move || {
        let messages = vec![user_msg("writer-1", 2)];
        save_messages_with_hooks(
            &thread_path,
            "stale-race",
            &thread_meta,
            &messages,
            || {
                validated_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
                Ok(())
            },
            write_messages_locked,
        )
    });

    validated_rx.recv().unwrap();

    let competitor_messages = vec![user_msg("writer-2", 3)];
    let competitor_meta = stale_meta;
    let competitor =
        thread::spawn(move || store.save("stale-race", &competitor_meta, &competitor_messages));

    resume_tx.send(()).unwrap();

    let first_result = paused_writer.join().unwrap();
    let second_result = competitor.join().unwrap();

    let conflict_count = usize::from(first_result.is_err()) + usize::from(second_result.is_err());
    assert_eq!(
        conflict_count, 1,
        "exactly one stale writer should conflict"
    );

    let (loaded_meta, loaded_messages) = JsonlSessionStore::new(dir.path().to_path_buf())
        .unwrap()
        .load("stale-race", None)
        .unwrap();
    assert_eq!(loaded_meta.sequence, 2);
    assert_eq!(loaded_messages.len(), 1);
}

#[test]
fn save_interrupt_requires_existing_session() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let err = store
        .save_interrupt(
            "missing",
            &InterruptState {
                interrupted_at: 1,
                pending_tool_calls: vec![],
                context_snapshot: vec![],
                system_prompt: "system".to_string(),
                model: swink_agent::ModelSpec::new("openai", "gpt-4o"),
            },
        )
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
}

/// Migrator that transforms the first `User` text content in every
/// `Message` entry from lower-case to upper-case. Used to detect whether
/// the migration pipeline actually ran against a given load path.
struct UppercasingMigrator;

impl crate::migrate::SessionMigrator for UppercasingMigrator {
    fn source_version(&self) -> u32 {
        0
    }
    fn target_version(&self) -> u32 {
        1
    }
    fn migrate(
        &self,
        _meta: &SessionMeta,
        entries: Vec<SessionEntry>,
    ) -> io::Result<Vec<SessionEntry>> {
        Ok(entries
            .into_iter()
            .map(|entry| match entry {
                SessionEntry::Message(LlmMessage::User(mut m)) => {
                    for block in &mut m.content {
                        if let swink_agent::ContentBlock::Text { text } = block {
                            *text = text.to_uppercase();
                        }
                    }
                    SessionEntry::Message(LlmMessage::User(m))
                }
                other => other,
            })
            .collect())
    }
}

/// Write a legacy (`version: 0`) session file containing both a
/// migrateable raw-`LlmMessage` line AND a custom-message envelope, then
/// assert that `load()` and `load_entries()` both observe the migrated
/// shape and that `load()` returns the custom wrapper unchanged.
///
/// Regression for #522: `load()` previously bypassed the configured
/// migrator pipeline, so it returned the raw pre-migration text while
/// `load_entries()` returned the migrated text.
#[test]
#[allow(clippy::too_many_lines)]
fn load_applies_migrators_identically_to_load_entries() {
    use swink_agent::{CustomMessage, CustomMessageRegistry};

    #[derive(Debug)]
    struct TestCustomMsg {
        data: String,
    }

    impl CustomMessage for TestCustomMsg {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn type_name(&self) -> Option<&str> {
            Some("TestCustomMsg")
        }
        fn to_json(&self) -> Option<serde_json::Value> {
            Some(serde_json::json!({ "data": self.data }))
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let id = "legacy";

    // Write a legacy-format file by hand: line 1 = meta @ version 0,
    // line 2 = raw LlmMessage (migrateable), line 3 = _custom envelope
    // (pass-through), line 4 = _state wrapper (pass-through, skipped by
    // load but must not break migration).
    let path = session_path(dir.path(), id);
    let now = now_utc();
    let meta = SessionMeta {
        id: id.to_string(),
        title: "legacy".to_string(),
        created_at: now,
        updated_at: now,
        version: 0,
        sequence: 0,
    };

    let raw_msg_line = serde_json::to_string(&LlmMessage::User(
        swink_agent::UserMessage::new(vec![swink_agent::ContentBlock::Text {
            text: "hello".to_string(),
        }])
        .with_timestamp(1),
    ))
    .unwrap();

    let custom_envelope = serde_json::json!({
        "type": "TestCustomMsg",
        "data": { "data": "custom-payload" },
        "_custom": true,
    });
    let custom_line = serde_json::to_string(&custom_envelope).unwrap();

    let state_line = serde_json::json!({
        "_state": true,
        "data": { "cursor": 9 },
    })
    .to_string();

    let contents = format!(
        "{}\n{raw_msg_line}\n{custom_line}\n{state_line}\n",
        serde_json::to_string(&meta).unwrap()
    );
    std::fs::write(&path, contents).unwrap();

    let store = JsonlSessionStore::new(dir.path().to_path_buf())
        .unwrap()
        .with_migrators(vec![Box::new(UppercasingMigrator)]);

    // load_entries() runs migrators — this is the pre-fix baseline.
    let (entries_meta, entries) = store.load_entries(id).unwrap();
    assert_eq!(entries_meta.version, crate::migrate::CURRENT_VERSION);
    let entry_msg = entries
        .iter()
        .find_map(SessionEntry::as_message)
        .expect("message entry present");
    if let LlmMessage::User(user) = entry_msg
        && let swink_agent::ContentBlock::Text { text } = &user.content[0]
    {
        assert_eq!(text, "HELLO", "load_entries must run migrator");
    } else {
        panic!("unexpected entry shape");
    }

    // load() must observe the same post-migration text AND still return
    // the custom wrapper unchanged (requires the registry).
    let mut registry = CustomMessageRegistry::new();
    registry.register(
        "TestCustomMsg",
        Box::new(|val: serde_json::Value| {
            let data = val
                .get("data")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing data".to_string())?;
            Ok(Box::new(TestCustomMsg {
                data: data.to_string(),
            }) as Box<dyn CustomMessage>)
        }),
    );

    let (load_meta, messages) = store.load(id, Some(&registry)).unwrap();
    assert_eq!(load_meta.version, crate::migrate::CURRENT_VERSION);

    let llm_text = messages
        .iter()
        .find_map(|m| match m {
            AgentMessage::Llm(LlmMessage::User(u)) => u.content.iter().find_map(|b| match b {
                swink_agent::ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            }),
            _ => None,
        })
        .expect("user message present");
    assert_eq!(
        llm_text, "HELLO",
        "load() must route through the migrator pipeline"
    );

    let custom = messages
        .iter()
        .find_map(|m| m.downcast_ref::<TestCustomMsg>().ok())
        .expect("custom wrapper must pass through load() unchanged");
    assert_eq!(custom.data, "custom-payload");
}

/// Without a registered migrator, a legacy-version file must still fail
/// `load()` with the same error that `load_entries()` emits — contract
/// consistency between the two APIs.
#[test]
fn load_rejects_legacy_version_without_migrator_like_load_entries() {
    let dir = tempfile::tempdir().unwrap();
    let id = "no-migrator";
    let path = session_path(dir.path(), id);
    let now = now_utc();
    let meta = SessionMeta {
        id: id.to_string(),
        title: "legacy".to_string(),
        created_at: now,
        updated_at: now,
        version: 0,
        sequence: 0,
    };
    let contents = format!("{}\n", serde_json::to_string(&meta).unwrap());
    std::fs::write(&path, contents).unwrap();

    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
    let load_err = store.load(id, None).unwrap_err();
    let entries_err = store.load_entries(id).unwrap_err();
    assert_eq!(load_err.kind(), io::ErrorKind::InvalidData);
    assert_eq!(entries_err.kind(), io::ErrorKind::InvalidData);
    assert!(load_err.to_string().contains("no migrator found"));
    assert!(entries_err.to_string().contains("no migrator found"));
}

#[test]
fn load_interrupt_ignores_orphan_file_without_session() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let orphan_path = interrupt_path(dir.path(), "orphan");
    std::fs::write(
        &orphan_path,
        serde_json::to_string(&InterruptState {
            interrupted_at: 2,
            pending_tool_calls: vec![],
            context_snapshot: vec![],
            system_prompt: "system".to_string(),
            model: swink_agent::ModelSpec::new("openai", "gpt-4o"),
        })
        .unwrap(),
    )
    .unwrap();

    assert!(store.load_interrupt("orphan").unwrap().is_none());
}
