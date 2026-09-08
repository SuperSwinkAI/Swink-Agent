//! Tests for `noop_tool`.
#![cfg(test)]

use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::tool::AgentTool;

fn test_state() -> Arc<std::sync::RwLock<crate::SessionState>> {
    Arc::new(std::sync::RwLock::new(crate::SessionState::new()))
}

#[test]
fn noop_tool_name_matches() {
    let tool = NoopTool::new("old_tool");
    assert_eq!(tool.name(), "old_tool");
}

#[test]
fn noop_tool_no_approval_required() {
    let tool = NoopTool::new("removed_tool");
    assert!(!tool.requires_approval());
}

#[tokio::test]
async fn noop_tool_returns_error() {
    let tool = NoopTool::new("deleted_tool");
    let result = tool
        .execute(
            "call_1",
            json!({"any": "args"}),
            CancellationToken::new(),
            None,
            test_state(),
            None,
        )
        .await;
    assert!(result.is_error);
    let crate::types::ContentBlock::Text { text } = &result.content[0] else {
        panic!("expected text content");
    };
    assert!(text.contains("deleted_tool"));
    assert!(text.contains("no longer available"));
}

#[tokio::test]
async fn noop_tool_ignores_arguments() {
    let tool = NoopTool::new("any");
    let result = tool
        .execute(
            "call_2",
            json!({"complex": {"nested": true}, "array": [1, 2, 3]}),
            CancellationToken::new(),
            None,
            test_state(),
            None,
        )
        .await;
    assert!(result.is_error);
}
