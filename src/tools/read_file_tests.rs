//! Tests for `read_file`.
#![cfg(test)]

use std::sync::{Arc, RwLock};

use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::SessionState;
use crate::types::ContentBlock;

fn result_text(result: &AgentToolResult) -> &str {
    match result.content.first() {
        Some(ContentBlock::Text { text }) => text.as_str(),
        _ => panic!("expected text content"),
    }
}

#[tokio::test]
async fn read_file_truncates_multibyte_content_on_char_boundary() {
    let temp = tempfile::NamedTempFile::new().unwrap();
    let content = "€".repeat((MAX_OUTPUT_BYTES / "€".len()) + 1);
    tokio::fs::write(temp.path(), content).await.unwrap();

    let result = ReadFileTool::new()
        .execute(
            "call-1",
            json!({ "path": temp.path().to_str().unwrap() }),
            CancellationToken::new(),
            None,
            Arc::new(RwLock::new(SessionState::new())),
            None,
        )
        .await;

    let text = result_text(&result);
    assert!(!result.is_error);
    assert!(text.contains("[truncated]"), "expected marker in: {text}");
    assert!(text.is_char_boundary(text.len()));
}

#[tokio::test]
async fn read_file_resolves_relative_path_against_execution_root() {
    let temp = tempfile::tempdir().unwrap();
    tokio::fs::write(temp.path().join("relative.txt"), "rooted")
        .await
        .unwrap();

    let result = ReadFileTool::new()
        .with_execution_root(temp.path())
        .execute(
            "call-1",
            json!({ "path": "relative.txt" }),
            CancellationToken::new(),
            None,
            Arc::new(RwLock::new(SessionState::new())),
            None,
        )
        .await;

    assert!(!result.is_error);
    assert_eq!(result_text(&result), "rooted");
}

#[tokio::test]
async fn read_file_rejects_relative_path_outside_execution_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    tokio::fs::create_dir(&root).await.unwrap();
    tokio::fs::write(temp.path().join("outside.txt"), "outside")
        .await
        .unwrap();

    let result = ReadFileTool::new()
        .with_execution_root(&root)
        .execute(
            "call-1",
            json!({ "path": "../outside.txt" }),
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
}
