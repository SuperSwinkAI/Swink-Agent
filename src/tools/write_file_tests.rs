//! Tests for `write_file`.
#![cfg(test)]

use std::sync::{Arc, RwLock};

use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::SessionState;

fn result_text(result: &AgentToolResult) -> &str {
    match result.content.first() {
        Some(ContentBlock::Text { text }) => text.as_str(),
        _ => panic!("expected text content"),
    }
}

#[tokio::test]
async fn approval_context_exposes_old_and_new_content_for_existing_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    tokio::fs::create_dir(&root).await.unwrap();
    tokio::fs::write(root.join("notes.txt"), "before\n")
        .await
        .unwrap();

    let context = WriteFileTool::new()
        .with_execution_root(&root)
        .approval_context(&json!({ "path": "notes.txt", "content": "after\n" }))
        .expect("existing file inside the root should yield approval context");

    assert_eq!(context["old_content"], "before\n");
    assert_eq!(context["new_content"], "after\n");
    assert_eq!(context["is_new_file"], false);
}

#[tokio::test]
async fn approval_context_marks_missing_file_as_new() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    tokio::fs::create_dir(&root).await.unwrap();

    let context = WriteFileTool::new()
        .with_execution_root(&root)
        .approval_context(&json!({ "path": "fresh.txt", "content": "hello\n" }))
        .expect("a not-yet-created file inside the root still yields context");

    assert_eq!(context["old_content"], "");
    assert_eq!(context["is_new_file"], true);
}

#[tokio::test]
async fn approval_context_refuses_path_outside_execution_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    tokio::fs::create_dir(&root).await.unwrap();
    tokio::fs::write(temp.path().join("outside.txt"), "secret\n")
        .await
        .unwrap();

    assert!(
        WriteFileTool::new()
            .with_execution_root(&root)
            .approval_context(&json!({ "path": "../outside.txt", "content": "x" }))
            .is_none(),
        "content outside the execution root must not leak into approval context"
    );
}

#[tokio::test]
async fn approval_context_returns_none_for_invalid_params() {
    assert!(
        WriteFileTool::new()
            .approval_context(&json!({ "path": "notes.txt" }))
            .is_none(),
        "missing content should not produce a diff preview"
    );
}

#[tokio::test]
async fn write_file_rejects_relative_path_outside_execution_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    tokio::fs::create_dir(&root).await.unwrap();
    let outside = temp.path().join("outside.txt");

    let result = WriteFileTool::new()
        .with_execution_root(&root)
        .execute(
            "call-1",
            json!({ "path": "../outside.txt", "content": "outside" }),
            CancellationToken::new(),
            None,
            Arc::new(RwLock::new(SessionState::new())),
            None,
        )
        .await;

    assert!(result.is_error);
    assert!(
        result_text(&result).contains("escapes execution root"),
        "unexpected result: {}",
        result_text(&result)
    );
    assert!(
        !tokio::fs::try_exists(&outside).await.unwrap(),
        "write escaped execution root"
    );
}
