//! Tests for `commands`.
#![cfg(test)]

use super::*;

// --- Not-a-command ---

#[test]
fn plain_text_is_not_a_command() {
    assert!(matches!(
        execute_command("hello world"),
        CommandResult::NotACommand
    ));
}

#[test]
fn empty_input_is_not_a_command() {
    assert!(matches!(execute_command(""), CommandResult::NotACommand));
}

#[test]
fn whitespace_only_is_not_a_command() {
    assert!(matches!(execute_command("   "), CommandResult::NotACommand));
}

// --- Hash commands ---

#[test]
fn hash_help_toggles_panel() {
    assert!(matches!(
        execute_command("#help"),
        CommandResult::ToggleHelp
    ));
}

#[test]
fn hash_clear_returns_clear() {
    assert!(matches!(execute_command("#clear"), CommandResult::Clear));
}

#[test]
fn hash_info_returns_feedback() {
    assert!(matches!(
        execute_command("#info"),
        CommandResult::Feedback(_)
    ));
}

#[test]
fn hash_copy_variants() {
    assert!(matches!(
        execute_command("#copy"),
        CommandResult::CopyToClipboard(ClipboardContent::Last)
    ));
    assert!(matches!(
        execute_command("#copy all"),
        CommandResult::CopyToClipboard(ClipboardContent::All)
    ));
    assert!(matches!(
        execute_command("#copy code"),
        CommandResult::CopyToClipboard(ClipboardContent::Code)
    ));
}

#[test]
fn hash_sessions_returns_list_sessions() {
    assert!(matches!(
        execute_command("#sessions"),
        CommandResult::ListSessions
    ));
}

#[test]
fn hash_save_returns_save_session() {
    assert!(matches!(
        execute_command("#save"),
        CommandResult::SaveSession
    ));
}

#[test]
fn hash_load_with_id() {
    match execute_command("#load abc123") {
        CommandResult::LoadSession(id) => assert_eq!(id, "abc123"),
        other => panic!("expected LoadSession, got {other:?}"),
    }
}

#[test]
fn hash_load_without_id_returns_feedback() {
    // "#load" alone (no trailing space) is treated as unknown command.
    match execute_command("#load") {
        CommandResult::Feedback(msg) => assert!(msg.contains("Unknown command")),
        other => panic!("expected Feedback, got {other:?}"),
    }
}

#[test]
fn hash_key_with_provider_and_key() {
    match execute_command("#key openai sk-abc123") {
        CommandResult::StoreKey { provider, key } => {
            assert_eq!(provider, "openai");
            assert_eq!(key, "sk-abc123");
        }
        other => panic!("expected StoreKey, got {other:?}"),
    }
}

#[test]
fn hash_key_without_key_returns_usage() {
    match execute_command("#key openai") {
        CommandResult::Feedback(msg) => assert!(msg.contains("Usage")),
        other => panic!("expected Feedback with usage, got {other:?}"),
    }
}

#[test]
fn hash_key_accepts_ascii_whitespace_separators() {
    match execute_command("#key\topenai\t sk-abc123") {
        CommandResult::StoreKey { provider, key } => {
            assert_eq!(provider, "openai");
            assert_eq!(key, "sk-abc123");
        }
        other => panic!("expected StoreKey, got {other:?}"),
    }
}

#[test]
fn hash_keys_returns_list_keys() {
    assert!(matches!(execute_command("#keys"), CommandResult::ListKeys));
}

#[test]
fn hash_approve_query() {
    assert!(matches!(
        execute_command("#approve"),
        CommandResult::QueryApprovalMode
    ));
}

#[test]
fn hash_approve_on() {
    assert!(matches!(
        execute_command("#approve on"),
        CommandResult::SetApprovalMode(ApprovalModeArg::On)
    ));
}

#[test]
fn hash_approve_off() {
    assert!(matches!(
        execute_command("#approve off"),
        CommandResult::SetApprovalMode(ApprovalModeArg::Off)
    ));
}

#[test]
fn hash_approve_smart() {
    assert!(matches!(
        execute_command("#approve smart"),
        CommandResult::SetApprovalMode(ApprovalModeArg::Smart)
    ));
}

#[test]
fn hash_approve_invalid_arg_returns_usage() {
    match execute_command("#approve maybe") {
        CommandResult::Feedback(msg) => {
            assert!(msg.contains("Usage"));
            assert!(msg.contains("smart"));
        }
        other => panic!("expected Feedback with usage, got {other:?}"),
    }
}

#[test]
fn hash_unknown_command_returns_feedback() {
    match execute_command("#nonexistent") {
        CommandResult::Feedback(msg) => {
            assert!(msg.contains("Unknown command"));
            assert!(msg.contains("#nonexistent"));
        }
        other => panic!("expected Feedback, got {other:?}"),
    }
}

// --- Slash commands ---

#[test]
fn slash_quit() {
    assert!(matches!(execute_command("/quit"), CommandResult::Quit));
}

#[test]
fn slash_q_alias() {
    assert!(matches!(execute_command("/q"), CommandResult::Quit));
}

#[test]
fn slash_model_is_unknown_command() {
    match execute_command("/model gpt-4o") {
        CommandResult::Feedback(msg) => assert!(msg.contains("Unknown command")),
        other => panic!("expected Feedback (unknown command), got {other:?}"),
    }
}

#[test]
fn slash_thinking_with_arg() {
    match execute_command("/thinking high") {
        CommandResult::SetThinking(level) => assert_eq!(level, ThinkingLevel::High),
        other => panic!("expected SetThinking, got {other:?}"),
    }
}

#[test]
fn slash_thinking_without_arg_returns_usage() {
    match execute_command("/thinking") {
        CommandResult::Feedback(msg) => assert!(msg.contains("Usage")),
        other => panic!("expected Feedback, got {other:?}"),
    }
}

#[test]
fn slash_thinking_invalid_arg_returns_usage() {
    match execute_command("/thinking maximum") {
        CommandResult::Feedback(msg) => {
            assert!(msg.contains("Usage"));
            assert!(msg.contains("extra-high"));
        }
        other => panic!("expected Feedback, got {other:?}"),
    }
}

#[test]
fn slash_system_with_arg() {
    match execute_command("/system You are a pirate.") {
        CommandResult::SetSystemPrompt(p) => assert_eq!(p, "You are a pirate."),
        other => panic!("expected SetSystemPrompt, got {other:?}"),
    }
}

#[test]
fn slash_system_without_arg_returns_usage() {
    match execute_command("/system") {
        CommandResult::Feedback(msg) => assert!(msg.contains("Usage")),
        other => panic!("expected Feedback, got {other:?}"),
    }
}

#[test]
fn slash_reset() {
    assert!(matches!(execute_command("/reset"), CommandResult::Reset));
}

#[test]
fn slash_compact() {
    assert!(matches!(
        execute_command("/compact"),
        CommandResult::Compact
    ));
}

#[test]
fn slash_editor() {
    assert!(matches!(
        execute_command("/editor"),
        CommandResult::OpenEditor
    ));
}

#[test]
fn slash_plan() {
    assert!(matches!(
        execute_command("/plan"),
        CommandResult::TogglePlanMode
    ));
}

#[test]
fn slash_usage() {
    assert!(matches!(
        execute_command("/usage"),
        CommandResult::ShowUsage
    ));
}

#[test]
fn slash_usage_ignores_trailing_args() {
    assert!(matches!(
        execute_command("/usage all"),
        CommandResult::ShowUsage
    ));
}

// --- split_command ---

#[test]
fn split_command_extracts_slash_name_and_args() {
    assert_eq!(split_command("/thinking high"), Some(("thinking", "high")));
}

#[test]
fn split_command_extracts_hash_name_and_args() {
    assert_eq!(split_command("#approve on"), Some(("approve", "on")));
}

#[test]
fn split_command_gives_empty_args_when_none_supplied() {
    assert_eq!(split_command("/usage"), Some(("usage", "")));
}

#[test]
fn split_command_trims_surrounding_whitespace() {
    assert_eq!(
        split_command("  /system  be terse  "),
        Some(("system", "be terse"))
    );
}

#[test]
fn split_command_rejects_plain_text_and_bare_sigils() {
    assert_eq!(split_command("hello"), None);
    assert_eq!(split_command("/"), None);
    assert_eq!(split_command("#"), None);
    assert_eq!(split_command(""), None);
}

#[test]
fn slash_unknown_command_returns_feedback() {
    match execute_command("/nonexistent") {
        CommandResult::Feedback(msg) => {
            assert!(msg.contains("Unknown command"));
            assert!(msg.contains("/nonexistent"));
        }
        other => panic!("expected Feedback, got {other:?}"),
    }
}

// --- Whitespace handling ---

#[test]
fn leading_trailing_whitespace_trimmed() {
    assert!(matches!(
        execute_command("  #clear  "),
        CommandResult::Clear
    ));
    assert!(matches!(execute_command("  /quit  "), CommandResult::Quit));
}

// --- Debug impl on enum variants ---

#[test]
fn approval_mode_arg_debug_and_eq() {
    assert_eq!(ApprovalModeArg::On, ApprovalModeArg::On);
    assert_ne!(ApprovalModeArg::On, ApprovalModeArg::Off);
    // Ensure Debug is implemented
    let _ = format!("{:?}", ApprovalModeArg::On);
}

// --- Sensitive input classification ---

#[test]
fn hash_key_with_provider_and_key_is_sensitive() {
    assert!(is_sensitive_input("#key openai sk-leak-sentinel-xyz"));
}

#[test]
fn hash_key_with_leading_whitespace_is_sensitive() {
    assert!(is_sensitive_input("   #key anthropic sk-ant-xyz   "));
}

#[test]
fn hash_key_with_ascii_whitespace_is_sensitive() {
    assert!(is_sensitive_input("#key\tanthropic\t sk-ant-xyz"));
}

#[test]
fn hash_key_only_is_not_sensitive() {
    // `#key` alone (no provider/key) exposes no secret.
    assert!(!is_sensitive_input("#key"));
}

#[test]
fn hash_key_with_provider_only_is_not_sensitive() {
    // No key present yet; treat as non-secret so the usage hint is
    // recallable via history.
    assert!(!is_sensitive_input("#key openai"));
}

#[test]
fn hash_key_args_rejects_bare_key_command() {
    assert_eq!(hash_key_args("key"), None);
    assert_eq!(hash_key_args(" key "), None);
}

#[test]
fn hash_keys_list_is_not_sensitive() {
    // `#keys` lists provider names and carries no secret material.
    assert!(!is_sensitive_input("#keys"));
}

#[test]
fn plain_text_is_not_sensitive() {
    assert!(!is_sensitive_input("hello world"));
    assert!(!is_sensitive_input("/help"));
    assert!(!is_sensitive_input("#help"));
}

#[test]
fn multiline_with_embedded_key_is_sensitive() {
    let input = "line one\n#key openai sk-embedded\nline three";
    assert!(is_sensitive_input(input));
}
