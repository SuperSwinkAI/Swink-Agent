//! Integration tests for JSONL format and corruption recovery (US5).

mod common;

use swink_agent_memory::{JsonlSessionStore, SessionStore};

use common::{assistant_message, sample_meta, user_message};

#[test]
fn jsonl_file_is_human_readable() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("readable_test", "Readable");
    let messages = vec![
        user_message("hello"),
        assistant_message("hi"),
        user_message("bye"),
    ];
    store.save("readable_test", &meta, &messages).unwrap();

    // Read raw file and verify each line is independently parseable JSON
    let content = std::fs::read_to_string(tmp.path().join("readable_test.jsonl")).unwrap();
    let lines: Vec<&str> = content.lines().collect();

    assert_eq!(lines.len(), 4); // 1 meta + 3 messages
    for (i, line) in lines.iter().enumerate() {
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(line);
        assert!(parsed.is_ok(), "line {i} is not valid JSON: {line}");
    }
}

#[test]
fn corrupted_line_recovers_remaining_messages() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("corrupt_test", "Corrupt test");
    let messages = vec![
        user_message("msg1"),
        user_message("msg2"),
        user_message("msg3"),
        user_message("msg4"),
        user_message("msg5"),
    ];
    store.save("corrupt_test", &meta, &messages).unwrap();

    // Read file, corrupt the 3rd message line (line index 3 = 4th line)
    let path = tmp.path().join("corrupt_test.jsonl");
    let content = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    assert_eq!(lines.len(), 6); // 1 meta + 5 messages
    lines[3] = "THIS IS CORRUPTED GARBAGE".to_string();
    std::fs::write(&path, lines.join("\n")).unwrap();

    // Load should recover 4 of 5 messages
    let (loaded_meta, loaded_msgs) = store.load("corrupt_test", None).unwrap();
    assert_eq!(loaded_meta.id, "corrupt_test");
    assert_eq!(loaded_msgs.len(), 4);
}

#[test]
fn all_message_lines_corrupted_returns_empty_messages() {
    let tmp = tempfile::tempdir().unwrap();

    let meta = sample_meta("all_corrupt", "All corrupt");
    let meta_json = serde_json::to_string(&meta).unwrap();

    // Write valid meta + all corrupted message lines
    let content = format!("{meta_json}\nGARBAGE1\nGARBAGE2\nGARBAGE3\n");
    std::fs::write(tmp.path().join("all_corrupt.jsonl"), &content).unwrap();

    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();
    let (loaded_meta, loaded_msgs) = store.load("all_corrupt", None).unwrap();
    assert_eq!(loaded_meta.id, "all_corrupt");
    assert!(loaded_msgs.is_empty());
}

#[test]
fn append_does_not_rewrite_file() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let meta = sample_meta("append_test", "Append test");
    let initial_msgs = vec![
        user_message("msg1"),
        user_message("msg2"),
        user_message("msg3"),
    ];
    store.save("append_test", &meta, &initial_msgs).unwrap();

    let path = tmp.path().join("append_test.jsonl");
    let size_before = std::fs::metadata(&path).unwrap().len();

    // Append 2 more messages
    let new_msgs = vec![user_message("msg4"), user_message("msg5")];
    store.append("append_test", &new_msgs).unwrap();

    let size_after = std::fs::metadata(&path).unwrap().len();
    assert!(
        size_after > size_before,
        "file should have grown: {size_before} -> {size_after}"
    );

    // Verify all 5 messages are present
    let (_, loaded_msgs) = store.load("append_test", None).unwrap();
    assert_eq!(loaded_msgs.len(), 5);
}
