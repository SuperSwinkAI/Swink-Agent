//! Per-hunk approve/reject review of a pending `write_file` approval.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use swink_agent::testing::ScriptedStreamFn;
use swink_agent::{ToolApproval, ToolApprovalRequest};

use crate::config::TuiConfig;

use super::super::*;
use super::helpers::*;

const OLD: &str = "a\nold1\nb\nold2\nc\n";
const NEW: &str = "a\nnew1\nb\nnew2\nc\n";

/// An `App` with an agent attached, so follow-up reporting has somewhere to go.
fn make_app() -> App {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);
    app
}

fn write_file_request(old: &str, new: &str, is_new_file: bool) -> ToolApprovalRequest {
    ToolApprovalRequest::new(
        "call_1",
        "write_file",
        serde_json::json!({"path": "/tmp/test.rs", "content": new}),
        true,
    )
    .with_context(serde_json::json!({
        "path": "/tmp/test.rs",
        "is_new_file": is_new_file,
        "old_content": old,
        "new_content": new,
    }))
}

/// Park a pending write_file approval on the app and return its response channel.
fn pending_write(
    app: &mut App,
    old: &str,
    new: &str,
) -> tokio::sync::oneshot::Receiver<ToolApproval> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.handle_approval_request(write_file_request(old, new, false), tx);
    rx
}

fn press(app: &mut App, code: KeyCode) {
    app.handle_key_event(KeyEvent::new(code, KeyModifiers::NONE));
}

#[tokio::test]
async fn h_opens_hunk_review_for_write_file_diff() {
    let mut app = make_app();
    let _rx = pending_write(&mut app, OLD, NEW);

    press(&mut app, KeyCode::Char('h'));

    let review = app.hunk_review.as_ref().expect("review should be open");
    assert_eq!(review.hunks.len(), 2);
    assert_eq!(review.cursor, 0);
    assert!(
        review.decisions.iter().all(Option::is_none),
        "no hunk should be decided yet"
    );
    // The approval stays pending until every hunk has a decision.
    assert!(app.pending_approval.is_some());
}

#[tokio::test]
async fn h_is_ignored_without_diff_context() {
    let mut app = make_app();
    let (tx, _rx) = tokio::sync::oneshot::channel();
    app.handle_approval_request(
        ToolApprovalRequest::new("call_1", "bash", serde_json::json!({"command": "ls"}), true),
        tx,
    );

    press(&mut app, KeyCode::Char('h'));

    assert!(app.hunk_review.is_none(), "no diff means no review");
    assert!(
        app.pending_approval.is_some(),
        "the plain approval prompt must remain"
    );
}

#[tokio::test]
async fn h_is_ignored_for_new_files() {
    let mut app = make_app();
    let (tx, _rx) = tokio::sync::oneshot::channel();
    app.handle_approval_request(write_file_request("", NEW, true), tx);

    press(&mut app, KeyCode::Char('h'));

    assert!(app.hunk_review.is_none());
    assert!(app.pending_approval.is_some());
}

#[tokio::test]
async fn reviewable_diff_is_advertised_only_when_a_review_is_available() {
    // Drives the `[h]unks` hint on the approval prompt.
    let mut app = make_app();
    assert!(
        !app.pending_approval_has_reviewable_diff(),
        "nothing is pending"
    );

    let _rx = pending_write(&mut app, OLD, NEW);
    assert!(app.pending_approval_has_reviewable_diff());

    // A new file is not reviewable.
    let mut app = make_app();
    let (tx, _rx) = tokio::sync::oneshot::channel();
    app.handle_approval_request(write_file_request("", NEW, true), tx);
    assert!(!app.pending_approval_has_reviewable_diff());

    // Neither is a tool call with no diff context.
    let mut app = make_app();
    let (tx, _rx) = tokio::sync::oneshot::channel();
    app.handle_approval_request(
        ToolApprovalRequest::new("call_1", "bash", serde_json::json!({"command": "ls"}), true),
        tx,
    );
    assert!(!app.pending_approval_has_reviewable_diff());
}

#[tokio::test]
async fn approving_every_hunk_sends_plain_approved() {
    let mut app = make_app();
    let rx = pending_write(&mut app, OLD, NEW);

    press(&mut app, KeyCode::Char('h'));
    press(&mut app, KeyCode::Char('y'));
    press(&mut app, KeyCode::Char('y'));

    assert!(app.hunk_review.is_none(), "review should have finalized");
    assert!(app.pending_approval.is_none());
    assert_eq!(rx.await.unwrap(), ToolApproval::Approved);
}

#[tokio::test]
async fn rejecting_every_hunk_sends_rejected() {
    let mut app = make_app();
    let rx = pending_write(&mut app, OLD, NEW);

    press(&mut app, KeyCode::Char('h'));
    press(&mut app, KeyCode::Char('n'));
    press(&mut app, KeyCode::Char('n'));

    assert!(app.hunk_review.is_none());
    assert_eq!(rx.await.unwrap(), ToolApproval::Rejected);
}

#[tokio::test]
async fn mixed_decisions_send_approved_with_merged_content() {
    let mut app = make_app();
    let rx = pending_write(&mut app, OLD, NEW);

    press(&mut app, KeyCode::Char('h'));
    press(&mut app, KeyCode::Char('y')); // apply hunk 1
    press(&mut app, KeyCode::Char('n')); // revert hunk 2

    match rx.await.unwrap() {
        ToolApproval::ApprovedWith(arguments) => {
            assert_eq!(
                arguments.get("content").and_then(serde_json::Value::as_str),
                Some("a\nnew1\nb\nold2\nc\n"),
                "only the approved hunk should be applied"
            );
            assert_eq!(
                arguments.get("path").and_then(serde_json::Value::as_str),
                Some("/tmp/test.rs"),
                "non-content arguments must be preserved"
            );
        }
        other => panic!("expected ApprovedWith, got {other:?}"),
    }
}

#[tokio::test]
async fn a_approves_all_remaining_hunks() {
    let mut app = make_app();
    let rx = pending_write(&mut app, OLD, NEW);

    press(&mut app, KeyCode::Char('h'));
    press(&mut app, KeyCode::Char('n')); // revert hunk 1
    press(&mut app, KeyCode::Char('a')); // apply everything left

    match rx.await.unwrap() {
        ToolApproval::ApprovedWith(arguments) => {
            assert_eq!(
                arguments.get("content").and_then(serde_json::Value::as_str),
                Some("a\nold1\nb\nnew2\nc\n"),
            );
        }
        other => panic!("expected ApprovedWith, got {other:?}"),
    }
}

#[tokio::test]
async fn a_on_first_hunk_approves_whole_write() {
    let mut app = make_app();
    let rx = pending_write(&mut app, OLD, NEW);

    press(&mut app, KeyCode::Char('h'));
    press(&mut app, KeyCode::Char('a'));

    assert_eq!(rx.await.unwrap(), ToolApproval::Approved);
}

#[tokio::test]
async fn esc_cancels_review_and_leaves_approval_pending() {
    let mut app = make_app();
    let _rx = pending_write(&mut app, OLD, NEW);

    press(&mut app, KeyCode::Char('h'));
    press(&mut app, KeyCode::Char('y'));
    press(&mut app, KeyCode::Esc);

    assert!(app.hunk_review.is_none(), "review should be discarded");
    assert!(
        app.pending_approval.is_some(),
        "the user must still answer the approval prompt"
    );
}

#[tokio::test]
async fn cancelled_review_can_be_reopened_from_scratch() {
    let mut app = make_app();
    let _rx = pending_write(&mut app, OLD, NEW);

    press(&mut app, KeyCode::Char('h'));
    press(&mut app, KeyCode::Char('y'));
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('h'));

    let review = app.hunk_review.as_ref().expect("review should reopen");
    assert_eq!(review.cursor, 0, "cursor should reset");
    assert!(
        review.decisions.iter().all(Option::is_none),
        "decisions from the abandoned review must not persist"
    );
}

#[tokio::test]
async fn rejected_hunks_are_reported_to_the_agent() {
    let mut app = make_app();
    // Approval always arrives mid-turn, so the follow-up is steered in at the
    // next turn boundary rather than starting a fresh prompt.
    app.status = AgentStatus::Running;
    let _rx = pending_write(&mut app, OLD, NEW);

    press(&mut app, KeyCode::Char('h'));
    press(&mut app, KeyCode::Char('y'));
    press(&mut app, KeyCode::Char('n'));

    // Locally the user sees which hunks were reverted...
    let notice = app
        .messages
        .iter()
        .find(|message| message.role == MessageRole::System)
        .expect("a system notice should be shown");
    assert!(
        notice.content.contains("hunk(s) 2") && notice.content.contains("/tmp/test.rs"),
        "notice should name the rejected hunk and file: {}",
        notice.content
    );

    // ...and the agent is told, so it does not assume its write landed intact.
    assert!(
        app.pending_steered
            .iter()
            .any(|steered| steered.contains("rejected hunk(s) 2")
                && steered.contains("/tmp/test.rs")),
        "agent should receive a follow-up describing the reverted hunks: {:?}",
        app.pending_steered
    );
}

#[tokio::test]
async fn fully_approved_review_does_not_message_the_agent() {
    let mut app = make_app();
    app.status = AgentStatus::Running;
    let _rx = pending_write(&mut app, OLD, NEW);

    press(&mut app, KeyCode::Char('h'));
    press(&mut app, KeyCode::Char('y'));
    press(&mut app, KeyCode::Char('y'));

    assert!(
        app.pending_steered.is_empty(),
        "nothing was reverted, so there is nothing to report: {:?}",
        app.pending_steered
    );
}

#[tokio::test]
async fn full_rejection_is_reported_to_the_agent() {
    let mut app = make_app();
    app.status = AgentStatus::Running;
    let _rx = pending_write(&mut app, OLD, NEW);

    press(&mut app, KeyCode::Char('h'));
    press(&mut app, KeyCode::Char('n'));
    press(&mut app, KeyCode::Char('n'));

    assert!(
        app.messages
            .iter()
            .any(|message| message.role == MessageRole::System
                && message.content.contains("left unchanged")),
        "user should see that nothing was written"
    );
}

#[tokio::test]
async fn non_object_arguments_fail_closed_to_rejected() {
    let mut app = make_app();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut request = write_file_request(OLD, NEW, false);
    // Arguments we cannot rewrite safely must never be approved as-is, since
    // that would apply the very hunks the user rejected.
    request.arguments = serde_json::json!("not-an-object");
    app.handle_approval_request(request, tx);

    press(&mut app, KeyCode::Char('h'));
    press(&mut app, KeyCode::Char('y'));
    press(&mut app, KeyCode::Char('n'));

    assert_eq!(rx.await.unwrap(), ToolApproval::Rejected);
}

#[tokio::test]
async fn hunk_review_keys_do_not_leak_to_the_approval_prompt() {
    let mut app = make_app();
    let _rx = pending_write(&mut app, OLD, NEW);

    press(&mut app, KeyCode::Char('h'));
    // 'a' inside a review means "apply remaining hunks", not "always trust".
    press(&mut app, KeyCode::Char('a'));

    assert!(
        !app.session_trusted_tools.contains("write_file"),
        "per-hunk 'a' must not add the tool to the session trust set"
    );
}
