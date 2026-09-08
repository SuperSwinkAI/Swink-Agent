//! Tests for `tool_panel`.
#![cfg(test)]

use super::*;

#[test]
fn start_tool_adds_to_active() {
    let mut panel = ToolPanel::new();
    panel.start_tool("t1".into(), "bash".into());
    assert_eq!(panel.active.len(), 1);
    assert_eq!(panel.active[0].id, "t1");
    assert_eq!(panel.active[0].name, "bash");
    assert!(panel.active[0].streamed_output.is_empty());
}

#[test]
fn end_tool_moves_to_completed() {
    let mut panel = ToolPanel::new();
    panel.start_tool("t1".into(), "bash".into());
    panel.end_tool("t1", false);
    assert!(panel.active.is_empty());
    assert_eq!(panel.completed.len(), 1);
    assert_eq!(panel.completed[0].id, "t1");
    assert!(!panel.completed[0].is_error);
}

#[test]
fn set_awaiting_approval_adds_to_pending() {
    let mut panel = ToolPanel::new();
    let args = serde_json::json!({"command": "rm -rf /"});
    panel.set_awaiting_approval("t1", "bash", &args);
    assert_eq!(panel.pending_approvals.len(), 1);
    assert_eq!(panel.pending_approvals[0].name, "bash");
}

#[test]
fn set_awaiting_approval_truncates_unicode_object_argument() {
    let mut panel = ToolPanel::new();
    let args = serde_json::json!({"command": "é".repeat(61)});

    panel.set_awaiting_approval("t1", "bash", &args);

    let summary = &panel.pending_approvals[0].arguments_summary;
    assert!(summary.starts_with("command="));
    assert!(summary.ends_with("..."));
    assert_eq!(summary.trim_start_matches("command=").chars().count(), 60);
}

#[test]
fn summarize_arguments_truncates_unicode_non_object_argument() {
    let args = serde_json::json!("é".repeat(61));

    let summary = summarize_arguments(&args);

    assert!(summary.ends_with("..."));
    assert_eq!(summary.chars().count(), 60);
}

#[test]
fn resolve_approval_moves_to_resolved() {
    let mut panel = ToolPanel::new();
    let args = serde_json::json!({"command": "ls"});
    panel.set_awaiting_approval("t1", "bash", &args);
    panel.resolve_approval("t1", true);
    assert!(panel.pending_approvals.is_empty());
    assert_eq!(panel.resolved_approvals.len(), 1);
    assert!(panel.resolved_approvals[0].approved);
}

#[test]
fn is_visible_when_has_active_tools() {
    let mut panel = ToolPanel::new();
    assert!(!panel.is_visible());
    panel.start_tool("t1".into(), "bash".into());
    assert!(panel.is_visible());
}

#[test]
fn is_visible_when_has_completed_tools() {
    let mut panel = ToolPanel::new();
    panel.start_tool("t1".into(), "bash".into());
    panel.end_tool("t1", false);
    assert!(panel.is_visible());
}

#[test]
fn not_visible_when_empty() {
    let panel = ToolPanel::new();
    assert!(!panel.is_visible());
}

#[test]
fn end_tool_out_of_order_concurrent() {
    let mut panel = ToolPanel::new();
    panel.start_tool("t1".into(), "bash".into());
    panel.start_tool("t2".into(), "read_file".into());
    panel.start_tool("t3".into(), "write_file".into());

    // Complete in reverse order (t3, t1, t2)
    panel.end_tool("t3", false);
    assert_eq!(panel.active.len(), 2);
    assert_eq!(panel.completed.len(), 1);
    assert_eq!(panel.completed[0].id, "t3");
    assert_eq!(panel.completed[0].name, "write_file");

    panel.end_tool("t1", true);
    assert_eq!(panel.active.len(), 1);
    assert_eq!(panel.completed.len(), 2);
    assert_eq!(panel.completed[1].id, "t1");
    assert!(panel.completed[1].is_error);

    panel.end_tool("t2", false);
    assert!(panel.active.is_empty());
    assert_eq!(panel.completed.len(), 3);
    assert_eq!(panel.completed[2].id, "t2");
}

#[test]
fn end_tool_unknown_id_is_noop() {
    let mut panel = ToolPanel::new();
    panel.start_tool("t1".into(), "bash".into());
    panel.end_tool("nonexistent", false);
    assert_eq!(panel.active.len(), 1);
    assert!(panel.completed.is_empty());
}

#[test]
fn update_tool_accumulates_incremental_output() {
    let mut panel = ToolPanel::new();
    panel.start_tool("t1".into(), "bash".into());

    panel.update_tool("t1", "bash", &AgentToolResult::text("line 1\n"));
    panel.update_tool("t1", "bash", &AgentToolResult::text("line 2"));

    assert_eq!(panel.active[0].streamed_output, "line 1\nline 2");
}

#[test]
fn update_tool_replaces_with_latest_snapshot() {
    let mut panel = ToolPanel::new();
    panel.start_tool("t1".into(), "bash".into());

    panel.update_tool("t1", "bash", &AgentToolResult::text("line 1"));
    panel.update_tool("t1", "bash", &AgentToolResult::text("line 1\nline 2"));

    assert_eq!(panel.active[0].streamed_output, "line 1\nline 2");
}

#[test]
fn update_tool_registers_missing_active_tool() {
    let mut panel = ToolPanel::new();

    panel.update_tool("t1", "bash", &AgentToolResult::text("working"));

    assert_eq!(panel.active.len(), 1);
    assert_eq!(panel.active[0].name, "bash");
    assert_eq!(panel.active[0].streamed_output, "working");
}

#[test]
fn height_capped_at_max() {
    let mut panel = ToolPanel::new();
    for i in 0..20 {
        panel.start_tool(format!("t{i}"), "tool".into());
    }
    assert!(panel.height() <= 10);
}
