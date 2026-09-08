//! Tests for `edit_file`.
#![cfg(test)]

use super::*;

// ── apply_op unit tests ──────────────────────────────────────────────────

#[test]
fn exact_single_replacement() {
    let content = "hello world\n";
    let op = EditOp {
        old_string: "world".into(),
        new_string: "Rust".into(),
        replace_all: false,
        line_hint: None,
    };
    assert_eq!(apply_op(content, &op).unwrap(), "hello Rust\n");
}

#[test]
fn normalised_trailing_whitespace_match() {
    // File has trailing spaces; old_string does not — should still match.
    let content = "fn foo() {   \n    let x = 1;\n}\n";
    let op = EditOp {
        old_string: "fn foo() {\n    let x = 1;\n}".into(),
        new_string: "fn foo() {\n    let x = 2;\n}".into(),
        replace_all: false,
        line_hint: None,
    };
    assert_eq!(
        apply_op(content, &op).unwrap(),
        "fn foo() {\n    let x = 2;\n}\n"
    );
}

#[test]
fn replace_all_occurrences() {
    let content = "foo bar foo baz foo\n";
    let op = EditOp {
        old_string: "foo".into(),
        new_string: "qux".into(),
        replace_all: true,
        line_hint: None,
    };
    assert_eq!(apply_op(content, &op).unwrap(), "qux bar qux baz qux\n");
}

#[test]
fn multiple_matches_without_hint_is_error() {
    let content = "fn foo() {}\nfn foo() {}\n";
    let op = EditOp {
        old_string: "fn foo() {}".into(),
        new_string: "fn bar() {}".into(),
        replace_all: false,
        line_hint: None,
    };
    let err = apply_op(content, &op).unwrap_err();
    assert!(err.contains("matched 2 times"), "unexpected error: {err}");
}

#[test]
fn line_hint_picks_closest_match() {
    // "fn foo() {}" appears on lines 1 and 3; hint=3 should pick line 3.
    let content = "fn foo() {}\nfn bar() {}\nfn foo() {}\n";
    let op = EditOp {
        old_string: "fn foo() {}".into(),
        new_string: "fn baz() {}".into(),
        replace_all: false,
        line_hint: Some(3),
    };
    assert_eq!(
        apply_op(content, &op).unwrap(),
        "fn foo() {}\nfn bar() {}\nfn baz() {}\n"
    );
}

#[test]
fn not_found_returns_error() {
    let content = "hello world\n";
    let op = EditOp {
        old_string: "missing".into(),
        new_string: "x".into(),
        replace_all: false,
        line_hint: None,
    };
    assert!(apply_op(content, &op).is_err());
}

#[test]
fn empty_old_string_is_error() {
    let op = EditOp {
        old_string: String::new(),
        new_string: "x".into(),
        replace_all: false,
        line_hint: None,
    };
    assert!(apply_op("anything", &op).is_err());
}

#[test]
fn multiple_edits_applied_in_order() {
    let mut content = "a b c\n".to_owned();
    let ops = [
        EditOp {
            old_string: "a".into(),
            new_string: "1".into(),
            replace_all: false,
            line_hint: None,
        },
        EditOp {
            old_string: "b".into(),
            new_string: "2".into(),
            replace_all: false,
            line_hint: None,
        },
        EditOp {
            old_string: "c".into(),
            new_string: "3".into(),
            replace_all: false,
            line_hint: None,
        },
    ];
    for op in &ops {
        content = apply_op(&content, op).unwrap();
    }
    assert_eq!(content, "1 2 3\n");
}

// ── sha256_hex ───────────────────────────────────────────────────────────

#[test]
fn sha256_hex_known_value() {
    // echo -n "abc" | sha256sum → ba7816bf…
    let digest = sha256_hex(b"abc");
    assert!(digest.starts_with("ba7816bf"), "got: {digest}");
    assert_eq!(digest.len(), 64);
}

// ── Integration: execute via tempfile ────────────────────────────────────

#[tokio::test]
async fn execute_edits_file_and_returns_diff() {
    use std::sync::{Arc, RwLock};

    use serde_json::json;

    use crate::SessionState;
    use crate::tool::AgentTool;

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    tokio::fs::write(&file, "hello world\n").await.unwrap();

    let tool = EditFileTool::new();
    let params = json!({
        "path": file.to_str().unwrap(),
        "edits": [{ "old_string": "world", "new_string": "Rust" }]
    });

    let result = tool
        .execute(
            "id",
            params,
            CancellationToken::new(),
            None,
            Arc::new(RwLock::new(SessionState::default())),
            None,
        )
        .await;

    assert!(!result.is_error);
    let on_disk = tokio::fs::read_to_string(&file).await.unwrap();
    assert_eq!(on_disk, "hello Rust\n");
    assert_eq!(result.details["old_content"], "hello world\n");
    assert_eq!(result.details["new_content"], "hello Rust\n");
}

#[tokio::test]
async fn execute_rejects_stale_hash() {
    use std::sync::{Arc, RwLock};

    use serde_json::json;

    use crate::SessionState;
    use crate::tool::AgentTool;

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    tokio::fs::write(&file, "hello world\n").await.unwrap();

    let tool = EditFileTool::new();
    let params = json!({
        "path": file.to_str().unwrap(),
        "edits": [{ "old_string": "world", "new_string": "Rust" }],
        "expected_hash": "0000000000000000000000000000000000000000000000000000000000000000"
    });

    let result = tool
        .execute(
            "id",
            params,
            CancellationToken::new(),
            None,
            Arc::new(RwLock::new(SessionState::default())),
            None,
        )
        .await;

    assert!(result.is_error);
    let text = match &result.content[0] {
        ContentBlock::Text { text } => text.clone(),
        _ => panic!("expected text block"),
    };
    assert!(text.contains("hash mismatch"), "got: {text}");
}

#[tokio::test]
async fn execute_rejects_relative_path_outside_execution_root() {
    use std::sync::{Arc, RwLock};

    use serde_json::json;

    use crate::SessionState;
    use crate::tool::AgentTool;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    tokio::fs::create_dir(&root).await.unwrap();
    let outside = dir.path().join("outside.txt");
    tokio::fs::write(&outside, "hello world\n").await.unwrap();

    let result = EditFileTool::new()
        .with_execution_root(&root)
        .execute(
            "id",
            json!({
                "path": "../outside.txt",
                "edits": [{ "old_string": "world", "new_string": "Rust" }]
            }),
            CancellationToken::new(),
            None,
            Arc::new(RwLock::new(SessionState::default())),
            None,
        )
        .await;

    assert!(result.is_error);
    let text = match &result.content[0] {
        ContentBlock::Text { text } => text.clone(),
        _ => panic!("expected text block"),
    };
    assert!(text.contains("escapes execution root"), "got: {text}");
    let on_disk = tokio::fs::read_to_string(&outside).await.unwrap();
    assert_eq!(on_disk, "hello world\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_same_target_edits_preserve_disjoint_changes() {
    use std::fmt::Write as _;
    use std::sync::{Arc, RwLock};

    use serde_json::json;
    use tokio::sync::Barrier;

    use crate::SessionState;
    use crate::tool::AgentTool;

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    let original = (0..16).fold(String::new(), |mut output, i| {
        writeln!(output, "line-{i:02}").unwrap();
        output
    });
    tokio::fs::write(&file, original).await.unwrap();

    let tool = Arc::new(EditFileTool::new());
    let barrier = Arc::new(Barrier::new(16));
    let mut handles = Vec::new();
    for i in 0..16 {
        let tool = Arc::clone(&tool);
        let barrier = Arc::clone(&barrier);
        let file = file.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            tool.execute(
                "id",
                json!({
                    "path": file.to_str().unwrap(),
                    "edits": [{
                        "old_string": format!("line-{i:02}"),
                        "new_string": format!("edited-{i:02}")
                    }]
                }),
                CancellationToken::new(),
                None,
                Arc::new(RwLock::new(SessionState::default())),
                None,
            )
            .await
        }));
    }

    for handle in handles {
        let result = handle.await.unwrap();
        assert!(
            !result.is_error,
            "unexpected edit failure: {:?}",
            result.content
        );
    }

    let on_disk = tokio::fs::read_to_string(&file).await.unwrap();
    for i in 0..16 {
        assert!(
            on_disk.contains(&format!("edited-{i:02}")),
            "missing concurrent edit {i}; final content:\n{on_disk}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_same_hash_edits_recheck_staleness_under_lock() {
    use std::sync::{Arc, RwLock};

    use serde_json::json;
    use tokio::sync::Barrier;

    use crate::SessionState;
    use crate::tool::AgentTool;

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    let original = "line-00\nline-01\n";
    tokio::fs::write(&file, original).await.unwrap();
    let expected_hash = sha256_hex(original.as_bytes());

    let tool = Arc::new(EditFileTool::new());
    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for i in 0..2 {
        let tool = Arc::clone(&tool);
        let barrier = Arc::clone(&barrier);
        let file = file.clone();
        let expected_hash = expected_hash.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            tool.execute(
                "id",
                json!({
                    "path": file.to_str().unwrap(),
                    "expected_hash": expected_hash,
                    "edits": [{
                        "old_string": format!("line-{i:02}"),
                        "new_string": format!("edited-{i:02}")
                    }]
                }),
                CancellationToken::new(),
                None,
                Arc::new(RwLock::new(SessionState::default())),
                None,
            )
            .await
        }));
    }

    let mut successes = 0;
    let mut stale_rejections = 0;
    for handle in handles {
        let result = handle.await.unwrap();
        if result.is_error {
            let text = ContentBlock::extract_text(&result.content);
            assert!(text.contains("hash mismatch"), "unexpected error: {text}");
            stale_rejections += 1;
        } else {
            successes += 1;
        }
    }

    assert_eq!(successes, 1);
    assert_eq!(stale_rejections, 1);
}
