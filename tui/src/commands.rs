//! Command system for the TUI.
//!
//! Hash commands (`#help`, `#clear`, etc.) are TUI-internal.
//! Slash commands (`/quit`, `/model`, etc.) affect agent configuration.

/// Result of parsing and executing a command.
#[derive(Debug)]
pub enum CommandResult {
    /// Command produced feedback to show in conversation.
    Feedback(String),
    /// Command requests quitting.
    Quit,
    /// Command requests clearing conversation.
    Clear,
    /// Command requests thinking level change.
    SetThinking(String),
    /// Command requests system prompt change.
    SetSystemPrompt(String),
    /// Command requests agent reset.
    Reset,
    /// Copy text to clipboard.
    CopyToClipboard(ClipboardContent),
    /// Save current session.
    SaveSession,
    /// Load a session by ID.
    LoadSession(String),
    /// List saved sessions.
    ListSessions,
    /// Store a credential.
    StoreKey { provider: String, key: String },
    /// List configured credentials.
    ListKeys,
    /// Set tool approval mode.
    SetApprovalMode(ApprovalModeArg),
    /// Query current approval mode.
    QueryApprovalMode,
    /// Open external editor for prompt composition.
    OpenEditor,
    /// Toggle plan mode.
    TogglePlanMode,
    /// Toggle the help side panel.
    ToggleHelp,
    /// Input was not a recognized command.
    NotACommand,
}

/// Parsed approval mode argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalModeArg {
    On,
    Off,
    Smart,
}

/// What to copy to clipboard.
#[derive(Debug, Clone, Copy)]
pub enum ClipboardContent {
    /// Last assistant message.
    Last,
    /// All conversation text.
    All,
    /// Last code block from assistant.
    Code,
}

/// Parse and execute a command string.
///
/// Returns `CommandResult` indicating what action to take.
pub fn execute_command(input: &str) -> CommandResult {
    let trimmed = input.trim();

    // Hash commands (TUI-internal)
    if let Some(cmd) = trimmed.strip_prefix('#') {
        return execute_hash_command(cmd.trim());
    }

    // Slash commands (agent config)
    if let Some(cmd) = trimmed.strip_prefix('/') {
        return execute_slash_command(cmd.trim());
    }

    CommandResult::NotACommand
}

fn execute_hash_command(cmd: &str) -> CommandResult {
    match cmd {
        "help" => CommandResult::ToggleHelp,
        "clear" => CommandResult::Clear,
        "info" => CommandResult::Feedback(String::new()), // Caller fills in session info
        "copy" => CommandResult::CopyToClipboard(ClipboardContent::Last),
        "copy all" => CommandResult::CopyToClipboard(ClipboardContent::All),
        "copy code" => CommandResult::CopyToClipboard(ClipboardContent::Code),
        "sessions" => CommandResult::ListSessions,
        "save" => CommandResult::SaveSession,
        "keys" => CommandResult::ListKeys,
        _ if cmd.starts_with("load ") => {
            let id = cmd.strip_prefix("load ").unwrap_or("").trim();
            if id.is_empty() {
                CommandResult::Feedback("Usage: #load <session-id>".to_string())
            } else {
                CommandResult::LoadSession(id.to_string())
            }
        }
        _ if cmd.starts_with("key ") => {
            let rest = cmd.strip_prefix("key ").unwrap_or("").trim();
            if let Some((provider, key)) = rest.split_once(' ') {
                CommandResult::StoreKey {
                    provider: provider.trim().to_string(),
                    key: key.trim().to_string(),
                }
            } else {
                CommandResult::Feedback("Usage: #key <provider> <api-key>".to_string())
            }
        }
        "approve" => CommandResult::QueryApprovalMode,
        "approve on" => CommandResult::SetApprovalMode(ApprovalModeArg::On),
        "approve off" => CommandResult::SetApprovalMode(ApprovalModeArg::Off),
        "approve smart" => CommandResult::SetApprovalMode(ApprovalModeArg::Smart),
        _ if cmd.starts_with("approve ") => {
            CommandResult::Feedback("Usage: #approve [on|off|smart]".to_string())
        }
        _ => CommandResult::Feedback(format!(
            "Unknown command: #{cmd}\nType #help for available commands."
        )),
    }
}

fn execute_slash_command(cmd: &str) -> CommandResult {
    let (name, args) = cmd.split_once(' ').unwrap_or((cmd, ""));
    let args = args.trim();

    match name {
        "quit" | "q" => CommandResult::Quit,
        "thinking" => {
            if args.is_empty() {
                CommandResult::Feedback("Usage: /thinking <off|low|medium|high>".to_string())
            } else {
                CommandResult::SetThinking(args.to_string())
            }
        }
        "system" => {
            if args.is_empty() {
                CommandResult::Feedback("Usage: /system <prompt>".to_string())
            } else {
                CommandResult::SetSystemPrompt(args.to_string())
            }
        }
        "reset" => CommandResult::Reset,
        "editor" => CommandResult::OpenEditor,
        "plan" => CommandResult::TogglePlanMode,
        _ => CommandResult::Feedback(format!(
            "Unknown command: /{name}\nType #help for available commands."
        )),
    }
}

#[cfg(test)]
mod tests {
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
            CommandResult::SetThinking(level) => assert_eq!(level, "high"),
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
}
