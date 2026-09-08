//! Command system for the TUI.
//!
//! Hash commands (`#help`, `#clear`, etc.) are TUI-internal.
//! Slash commands (`/quit`, `/thinking`, etc.) affect agent configuration.

use swink_agent::ThinkingLevel;

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
    SetThinking(ThinkingLevel),
    /// Command requests system prompt change.
    SetSystemPrompt(String),
    /// Command requests agent reset.
    Reset,
    /// Command requests manual context compaction.
    Compact,
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
    /// Revoke session trust for a specific tool.
    UntrustTool(String),
    /// Revoke all session trust.
    UntrustAll,
    /// Show the per-turn token/cost breakdown.
    ShowUsage,
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

/// Split a `#`/`/`-prefixed input into its bare command name and argument
/// string.
///
/// Returns `None` for plain text and for a bare sigil. The name is the first
/// whitespace-delimited token after the sigil; the arguments are everything
/// after it, trimmed. Used to match input against host-registered commands
/// before consulting the built-in table.
pub fn split_command(input: &str) -> Option<(&str, &str)> {
    let trimmed = input.trim();
    let body = trimmed
        .strip_prefix('#')
        .or_else(|| trimmed.strip_prefix('/'))?
        .trim();
    if body.is_empty() {
        return None;
    }
    let (name, args) = body
        .split_once(|c: char| c.is_whitespace())
        .unwrap_or((body, ""));
    Some((name, args.trim()))
}

/// Classify whether the given input carries a user-supplied secret.
///
/// The classification runs BEFORE the input is persisted to the editor's
/// history buffer, so that callers can skip history persistence for sensitive
/// submissions. Currently this covers `#key <provider> <api-key>` entries;
/// other hash/slash commands are treated as non-sensitive.
///
/// Detection is intentionally permissive: any line (in possibly multi-line
/// input) whose trimmed content starts with `#key ` followed by both a
/// provider and a key marks the whole submission as sensitive so that the
/// full entry is withheld from history, not just the offending line.
#[must_use]
pub fn is_sensitive_input(input: &str) -> bool {
    input.lines().any(|line| {
        line.trim_start()
            .strip_prefix('#')
            .and_then(hash_key_args)
            .and_then(parse_key_provider_and_value)
            .is_some()
    })
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
        _ if let Some(args) = hash_key_args(cmd) => {
            if let Some((provider, key)) = parse_key_provider_and_value(args) {
                CommandResult::StoreKey {
                    provider: provider.to_string(),
                    key: key.to_string(),
                }
            } else {
                CommandResult::Feedback("Usage: #key <provider> <api-key>".to_string())
            }
        }
        "approve" => CommandResult::QueryApprovalMode,
        "approve on" => CommandResult::SetApprovalMode(ApprovalModeArg::On),
        "approve off" => CommandResult::SetApprovalMode(ApprovalModeArg::Off),
        "approve smart" => CommandResult::SetApprovalMode(ApprovalModeArg::Smart),
        "approve untrust" => CommandResult::UntrustAll,
        _ if cmd.starts_with("approve untrust ") => {
            let tool_name = cmd.strip_prefix("approve untrust ").unwrap_or("").trim();
            if tool_name.is_empty() {
                CommandResult::UntrustAll
            } else {
                CommandResult::UntrustTool(tool_name.to_string())
            }
        }
        _ if cmd.starts_with("approve ") => {
            CommandResult::Feedback("Usage: #approve [on|off|smart|untrust [tool]]".to_string())
        }
        _ => CommandResult::Feedback(format!(
            "Unknown command: #{cmd}\nType #help for available commands."
        )),
    }
}

fn hash_key_args(cmd: &str) -> Option<&str> {
    if cmd.contains(['\r', '\n']) {
        return None;
    }

    let cmd = cmd.trim();
    let after_key = cmd.strip_prefix("key")?;
    if after_key.is_empty() {
        return None;
    }

    after_key.strip_prefix(|c: char| c.is_ascii_whitespace())
}

fn parse_key_provider_and_value(args: &str) -> Option<(&str, &str)> {
    let args = args.trim_matches(|c: char| c.is_ascii_whitespace());
    let split = args.find(|c: char| c.is_ascii_whitespace())?;
    let provider = &args[..split];
    let key = args[split..].trim_matches(|c: char| c.is_ascii_whitespace());

    if provider.is_empty() || key.is_empty() {
        None
    } else {
        Some((provider, key))
    }
}

fn execute_slash_command(cmd: &str) -> CommandResult {
    let (name, args) = cmd.split_once(' ').unwrap_or((cmd, ""));
    let args = args.trim();

    match name {
        "quit" | "q" => CommandResult::Quit,
        "thinking" => {
            if args.is_empty() {
                CommandResult::Feedback(
                    "Usage: /thinking <off|minimal|low|medium|high|extra-high>".to_string(),
                )
            } else {
                parse_thinking_level(args).map_or_else(
                    || {
                        CommandResult::Feedback(
                            "Usage: /thinking <off|minimal|low|medium|high|extra-high>".to_string(),
                        )
                    },
                    CommandResult::SetThinking,
                )
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
        "compact" => CommandResult::Compact,
        "editor" => CommandResult::OpenEditor,
        "plan" => CommandResult::TogglePlanMode,
        "usage" => CommandResult::ShowUsage,
        _ => CommandResult::Feedback(format!(
            "Unknown command: /{name}\nType #help for available commands."
        )),
    }
}

fn parse_thinking_level(level: &str) -> Option<ThinkingLevel> {
    match level {
        "off" => Some(ThinkingLevel::Off),
        "minimal" => Some(ThinkingLevel::Minimal),
        "low" => Some(ThinkingLevel::Low),
        "medium" => Some(ThinkingLevel::Medium),
        "high" => Some(ThinkingLevel::High),
        "extra-high" => Some(ThinkingLevel::ExtraHigh),
        _ => None,
    }
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
