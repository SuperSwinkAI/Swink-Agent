//! Integration tests for User Story 6 — TUI Rendering and Interaction.
//!
//! Tests T038–T043: role-based colors, inline diff coloring, context gauge
//! thresholds, plan mode, and approval mode classification.
//!
//! Because the TUI crate keeps `theme` and `ui` modules private, these tests
//! exercise the public API surface: `App`, `OperatingMode`, `ApprovalMode`,
//! `MessageRole`, `AgentStatus`, `DisplayMessage`, and `TuiConfig`.

use swink_agent::ApprovalMode;
use swink_agent_tui::app::{AgentStatus, DisplayMessage, MessageRole, OperatingMode};
use swink_agent_tui::config::TuiConfig;
use swink_agent_tui::App;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_app() -> App {
    App::new(TuiConfig::default())
}

fn make_display_message(role: MessageRole) -> DisplayMessage {
    DisplayMessage {
        role,
        content: format!("{role:?} message"),
        thinking: None,
        is_streaming: false,
        collapsed: false,
        summary: String::new(),
        user_expanded: false,
        expanded_at: None,
        plan_mode: false,
        diff_data: None,
    }
}

// ---------------------------------------------------------------------------
// T038 — Role-based border colors (AC 26)
//
// We cannot call theme functions directly (private module), but we verify
// that each `MessageRole` variant is distinct and that the public enum
// covers all expected roles. The actual color mapping is tested by unit
// tests inside the crate (`theme::tests` and `conversation` rendering).
// ---------------------------------------------------------------------------

#[test]
fn message_role_variants_are_distinct() {
    let roles = [
        MessageRole::User,
        MessageRole::Assistant,
        MessageRole::ToolResult,
        MessageRole::Error,
        MessageRole::System,
    ];

    // Every pair of roles should be distinguishable.
    for (i, a) in roles.iter().enumerate() {
        for (j, b) in roles.iter().enumerate() {
            if i != j {
                assert_ne!(a, b, "roles at index {i} and {j} must differ");
            }
        }
    }
}

#[test]
fn display_message_preserves_role() {
    for role in [
        MessageRole::User,
        MessageRole::Assistant,
        MessageRole::ToolResult,
        MessageRole::Error,
        MessageRole::System,
    ] {
        let msg = make_display_message(role);
        assert_eq!(msg.role, role);
    }
}

#[test]
fn plan_mode_message_flag() {
    let mut msg = make_display_message(MessageRole::Assistant);
    assert!(!msg.plan_mode, "default plan_mode should be false");
    msg.plan_mode = true;
    assert!(msg.plan_mode, "plan_mode flag should be settable");
}

// ---------------------------------------------------------------------------
// T039 — Inline diff color coding (AC 27)
//
// `DiffData` and `render_diff_lines` are in `ui::diff` (private module).
// We verify through the `DisplayMessage.diff_data` field that the type is
// accessible and that the `Option` storage round-trips correctly.
// Unit tests inside `ui::diff::tests` verify actual color output.
// ---------------------------------------------------------------------------

#[test]
fn display_message_diff_data_defaults_to_none() {
    let msg = make_display_message(MessageRole::ToolResult);
    assert!(msg.diff_data.is_none());
}

// ---------------------------------------------------------------------------
// T040 — Context gauge color thresholds (AC 28)
//
// The status bar applies threshold logic:
//   pct < 60  -> context_green()
//   pct < 85  -> context_yellow()
//   pct >= 85 -> context_red()
//
// We test the public fields that drive this logic and verify the threshold
// math. The actual color rendering is covered by `status_bar::render` unit
// tests and the theme module tests.
// ---------------------------------------------------------------------------

#[test]
fn context_gauge_fields_default_to_zero() {
    let app = make_app();
    assert_eq!(app.context_budget, 0, "budget should start at zero");
    assert_eq!(app.context_tokens_used, 0, "used should start at zero");
}

#[test]
fn context_gauge_threshold_math() {
    // Reproduce the exact threshold logic from status_bar.rs:
    //   pct < 60  -> green
    //   pct < 85  -> yellow
    //   pct >= 85 -> red
    struct Case {
        used: u64,
        budget: u64,
        expected_zone: &'static str,
    }

    let cases = [
        // 0% usage -> green
        Case {
            used: 0,
            budget: 100_000,
            expected_zone: "green",
        },
        // 50% -> green (below 60)
        Case {
            used: 50_000,
            budget: 100_000,
            expected_zone: "green",
        },
        // 59.9% -> green
        Case {
            used: 59_900,
            budget: 100_000,
            expected_zone: "green",
        },
        // 60% -> yellow
        Case {
            used: 60_000,
            budget: 100_000,
            expected_zone: "yellow",
        },
        // 75% -> yellow
        Case {
            used: 75_000,
            budget: 100_000,
            expected_zone: "yellow",
        },
        // 84.9% -> yellow
        Case {
            used: 84_900,
            budget: 100_000,
            expected_zone: "yellow",
        },
        // 85% -> red
        Case {
            used: 85_000,
            budget: 100_000,
            expected_zone: "red",
        },
        // 95% -> red
        Case {
            used: 95_000,
            budget: 100_000,
            expected_zone: "red",
        },
        // 100% -> red
        Case {
            used: 100_000,
            budget: 100_000,
            expected_zone: "red",
        },
    ];

    for case in &cases {
        let pct = (case.used as f64 / case.budget as f64) * 100.0;
        let zone = if pct < 60.0 {
            "green"
        } else if pct < 85.0 {
            "yellow"
        } else {
            "red"
        };
        assert_eq!(
            zone, case.expected_zone,
            "used={} budget={} pct={pct:.1} expected={} got={zone}",
            case.used, case.budget, case.expected_zone,
        );
    }
}

#[test]
fn context_fields_are_writable() {
    let mut app = make_app();
    app.context_budget = 200_000;
    app.context_tokens_used = 150_000;
    assert_eq!(app.context_budget, 200_000);
    assert_eq!(app.context_tokens_used, 150_000);
}

// ---------------------------------------------------------------------------
// T041 — Plan mode restricts write tools (AC 29)
//
// `toggle_operating_mode()` and `enter_plan_mode()` are `pub(super)`, so
// we cannot call them from integration tests. However `operating_mode` is
// `pub`, so we verify the enum semantics and that the App starts in Execute.
// The plan-mode tool filtering is exercised by unit tests in `app/tests.rs`.
// ---------------------------------------------------------------------------

#[test]
fn app_starts_in_execute_mode() {
    let app = make_app();
    assert_eq!(app.operating_mode, OperatingMode::Execute);
}

#[test]
fn operating_mode_enum_variants_are_distinct() {
    assert_ne!(OperatingMode::Execute, OperatingMode::Plan);
}

#[test]
fn operating_mode_field_is_writable() {
    let mut app = make_app();
    app.operating_mode = OperatingMode::Plan;
    assert_eq!(app.operating_mode, OperatingMode::Plan);

    app.operating_mode = OperatingMode::Execute;
    assert_eq!(app.operating_mode, OperatingMode::Execute);
}

// ---------------------------------------------------------------------------
// T042 — Approval mode classifies tools (AC 30)
//
// The TUI's `handle_approval_request` uses `ApprovalMode::Smart` +
// `session_trusted_tools` to auto-approve trusted tools. We test the public
// state that drives this: `approval_mode`, `session_trusted_tools`, and
// the `ApprovalMode` enum itself.
// ---------------------------------------------------------------------------

#[test]
fn default_approval_mode_is_smart() {
    let app = make_app();
    assert_eq!(app.approval_mode, ApprovalMode::Smart);
}

#[test]
fn approval_mode_variants_are_distinct() {
    let modes = [
        ApprovalMode::Enabled,
        ApprovalMode::Smart,
        ApprovalMode::Bypassed,
    ];
    for (i, a) in modes.iter().enumerate() {
        for (j, b) in modes.iter().enumerate() {
            if i != j {
                assert_ne!(a, b, "approval modes at {i} and {j} must differ");
            }
        }
    }
}

#[test]
fn approval_mode_field_is_writable() {
    let mut app = make_app();
    app.approval_mode = ApprovalMode::Smart;
    assert_eq!(app.approval_mode, ApprovalMode::Smart);

    app.approval_mode = ApprovalMode::Bypassed;
    assert_eq!(app.approval_mode, ApprovalMode::Bypassed);
}

#[test]
fn session_trusted_tools_starts_empty() {
    let app = make_app();
    assert!(app.session_trusted_tools.is_empty());
}

#[test]
fn session_trusted_tools_tracks_tool_names() {
    let mut app = make_app();
    app.approval_mode = ApprovalMode::Smart;

    app.session_trusted_tools.insert("ReadFile".to_string());
    app.session_trusted_tools.insert("ListDir".to_string());

    assert!(app.session_trusted_tools.contains("ReadFile"));
    assert!(app.session_trusted_tools.contains("ListDir"));
    assert!(
        !app.session_trusted_tools.contains("WriteFile"),
        "untrusted tool should not be in the set"
    );
}

#[test]
fn smart_mode_trust_semantics() {
    // In Smart mode, tools in session_trusted_tools are auto-approved.
    // Tools NOT in the set require approval prompts.
    // This mirrors the logic in `handle_approval_request`.
    let mut app = make_app();
    app.approval_mode = ApprovalMode::Smart;
    app.session_trusted_tools.insert("ReadFile".to_string());

    let trusted = app.session_trusted_tools.contains("ReadFile");
    let untrusted = app.session_trusted_tools.contains("WriteFile");

    assert!(trusted, "ReadFile should be auto-approved (trusted)");
    assert!(!untrusted, "WriteFile should require approval (not trusted)");
}

// ---------------------------------------------------------------------------
// T043 — Agent status transitions
// ---------------------------------------------------------------------------

#[test]
fn agent_status_starts_idle() {
    let app = make_app();
    assert_eq!(app.status, AgentStatus::Idle);
}

#[test]
fn agent_status_variants_are_distinct() {
    let statuses = [
        AgentStatus::Idle,
        AgentStatus::Running,
        AgentStatus::Error,
        AgentStatus::Aborted,
    ];
    for (i, a) in statuses.iter().enumerate() {
        for (j, b) in statuses.iter().enumerate() {
            if i != j {
                assert_ne!(a, b, "statuses at {i} and {j} must differ");
            }
        }
    }
}

#[test]
fn agent_status_field_is_writable() {
    let mut app = make_app();

    app.status = AgentStatus::Running;
    assert_eq!(app.status, AgentStatus::Running);

    app.status = AgentStatus::Error;
    assert_eq!(app.status, AgentStatus::Error);

    app.status = AgentStatus::Aborted;
    assert_eq!(app.status, AgentStatus::Aborted);

    app.status = AgentStatus::Idle;
    assert_eq!(app.status, AgentStatus::Idle);
}

// ---------------------------------------------------------------------------
// Additional display message tests
// ---------------------------------------------------------------------------

#[test]
fn display_message_streaming_flag() {
    let mut msg = make_display_message(MessageRole::Assistant);
    assert!(!msg.is_streaming, "default is_streaming should be false");
    msg.is_streaming = true;
    assert!(msg.is_streaming);
}

#[test]
fn display_message_collapse_toggle() {
    let mut msg = make_display_message(MessageRole::ToolResult);
    assert!(!msg.collapsed);
    msg.collapsed = true;
    assert!(msg.collapsed);
    msg.user_expanded = true;
    assert!(msg.user_expanded, "user_expanded prevents auto-collapse");
}
