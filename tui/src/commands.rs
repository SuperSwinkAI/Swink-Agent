//! Command system for the TUI.
//!
//! Hash commands (`#help`, `#clear`, etc.) are TUI-internal.
//! Slash commands (`/quit`, `/model`, etc.) affect agent configuration.

/// Result of parsing and executing a command.
pub enum CommandResult {
    /// Command produced feedback to show in conversation.
    Feedback(String),
    /// Command requests quitting.
    Quit,
    /// Command requests clearing conversation.
    Clear,
    /// Command requests model change.
    SetModel(String),
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
    /// Input was not a recognized command.
    NotACommand,
}

/// Parsed approval mode argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalModeArg {
    On,
    Off,
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
        "help" => CommandResult::Feedback(help_text()),
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
        _ if cmd.starts_with("approve ") => {
            CommandResult::Feedback("Usage: #approve [on|off]".to_string())
        }
        _ => CommandResult::Feedback(format!("Unknown command: #{cmd}\nType #help for available commands.")),
    }
}

fn execute_slash_command(cmd: &str) -> CommandResult {
    let (name, args) = cmd.split_once(' ').unwrap_or((cmd, ""));
    let args = args.trim();

    match name {
        "quit" | "q" => CommandResult::Quit,
        "model" => {
            if args.is_empty() {
                CommandResult::Feedback("Usage: /model <model-id>".to_string())
            } else {
                CommandResult::SetModel(args.to_string())
            }
        }
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
        _ => CommandResult::Feedback(format!("Unknown command: /{name}\nType #help for available commands.")),
    }
}

fn help_text() -> String {
    "\
╭─── Key Bindings ────────────────────────╮
│ Enter          Submit message            │
│ Shift+Enter    New line                  │
│ Ctrl+Q         Quit                      │
│ Ctrl+C         Quit (idle) / Abort (run) │
│ Tab            Toggle focus              │
│ Up/Down        Scroll / History          │
│ PageUp/Down    Scroll page               │
│ Home/Ctrl+A    Start of line             │
│ End/Ctrl+E     End of line               │
╰──────────────────────────────────────────╯
╭─── # Commands (TUI) ───────────────────╮
│ #help       Show this help              │
│ #clear      Clear conversation          │
│ #info       Session info                │
│ #copy       Copy last response          │
│ #copy all   Copy full conversation      │
│ #copy code  Copy last code block        │
│ #sessions   List saved sessions         │
│ #save       Save current session        │
│ #load <id>  Load a saved session        │
│ #keys       List configured providers   │
│ #key <p> <k> Store API key for provider │
│ #approve      Show approval mode         │
│ #approve on   Enable tool approval       │
│ #approve off  Disable tool approval      │
╰──────────────────────────────────────────╯
╭─── / Commands (Agent) ──────────────────╮
│ /quit       Exit                        │
│ /model <id> Switch model                │
│ /thinking   Set thinking level          │
│ /system     Update system prompt        │
│ /reset      Reset agent state           │
╰──────────────────────────────────────────╯"
        .to_string()
}
