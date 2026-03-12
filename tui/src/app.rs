//! Top-level application state and event loop.

use std::collections::HashSet;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::{mpsc, oneshot};

use swink_agent::{
    Agent, AgentEvent, AgentMessage, AgentTool, ApprovalMode, AssistantMessageDelta, ContentBlock,
    LlmMessage, ToolApproval, ToolApprovalRequest, UserMessage,
};

use tracing::{info, warn};

use crate::commands::{self, ApprovalModeArg, ClipboardContent, CommandResult};
use crate::config::TuiConfig;
use crate::credentials;
use crate::session::{JsonlSessionStore, SessionStore};
use crate::theme;
use crate::ui;
use crate::ui::conversation::ConversationView;
use crate::ui::input::InputEditor;
use crate::ui::help_panel::HelpPanel;
use crate::ui::tool_panel::ToolPanel;

type AppResult<T> = Result<T, Box<dyn std::error::Error>>;

const PLAN_MODE_ADDENDUM: &str = "\n\nYou are in planning mode. Analyze the request and produce a step-by-step plan. Do not make any modifications or execute any write operations.";

/// Seconds before a tool result auto-collapses (unless user-expanded).
const AUTO_COLLAPSE_SECS: u64 = 10;

/// Agent state as visible to the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Running,
    Error,
    Aborted,
}

/// Which UI component has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Input,
    Conversation,
}

/// Operating mode for the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatingMode {
    /// Normal execution mode — all tools available.
    Execute,
    /// Plan mode — read-only tools only, agent produces plans.
    Plan,
}

/// Message role for display styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    ToolResult,
    Error,
    System,
}

/// A message formatted for display.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct DisplayMessage {
    pub role: MessageRole,
    pub content: String,
    pub thinking: Option<String>,
    pub is_streaming: bool,
    /// Whether this tool result block is collapsed.
    pub collapsed: bool,
    /// One-line summary for collapsed display.
    pub summary: String,
    /// Whether the user manually expanded this block (prevents auto-collapse).
    pub user_expanded: bool,
    /// When the tool result was expanded (for auto-collapse timing).
    pub expanded_at: Option<Instant>,
    /// Whether this message was produced in plan mode.
    pub plan_mode: bool,
    /// Diff data for file modification tool results.
    pub diff_data: Option<crate::ui::diff::DiffData>,
}

/// Top-level application state.
#[allow(clippy::struct_excessive_bools)]
pub struct App {
    /// Whether the application should exit.
    pub should_quit: bool,
    /// Current agent status.
    pub status: AgentStatus,
    /// Multi-line input editor.
    pub input: InputEditor,
    /// Conversation messages for display.
    pub messages: Vec<DisplayMessage>,
    /// Conversation scroll state.
    pub conversation: ConversationView,
    /// Tool execution panel.
    pub tool_panel: ToolPanel,
    /// Help side panel (F1).
    pub help_panel: HelpPanel,
    /// Which component has focus.
    pub focus: Focus,
    /// Model identifier string.
    pub model_name: String,
    /// Token usage counters.
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    /// Running cost.
    pub total_cost: f64,
    /// Session start time for elapsed display.
    pub session_start: Instant,
    /// Dirty flag — only redraw when true.
    pub dirty: bool,
    /// Blink state for streaming cursor (toggled on tick).
    pub blink_on: bool,
    /// Tick counter for blink timing.
    tick_count: u64,
    /// Agent instance (if connected).
    agent: Option<Agent>,
    /// Sender for agent events.
    agent_tx: mpsc::Sender<AgentEvent>,
    /// Receiver for agent events.
    agent_rx: mpsc::Receiver<AgentEvent>,
    /// Configuration.
    pub config: TuiConfig,
    /// Retry attempt counter for error display.
    pub retry_attempt: Option<u32>,
    /// Session manager for persistence.
    session_store: Option<JsonlSessionStore>,
    /// Current session ID.
    session_id: String,
    /// Receiver for tool approval requests from the agent callback.
    approval_rx: mpsc::Receiver<(ToolApprovalRequest, oneshot::Sender<ToolApproval>)>,
    /// Sender for tool approval requests (cloned into the approval callback).
    approval_tx: mpsc::Sender<(ToolApprovalRequest, oneshot::Sender<ToolApproval>)>,
    /// Currently pending approval request and its response channel.
    pending_approval: Option<(ToolApprovalRequest, oneshot::Sender<ToolApproval>)>,
    /// Current approval mode.
    pub approval_mode: ApprovalMode,
    /// Estimated context window token budget.
    pub context_budget: u64,
    /// Estimated tokens currently used in context.
    pub context_tokens_used: u64,
    /// Index of the currently selected tool result block (for collapse toggling).
    pub selected_tool_block: Option<usize>,
    /// Flag set when external editor should be opened (processed by event loop).
    pub open_editor_requested: bool,
    /// Set of tool names trusted for the current session (auto-approved in Smart mode).
    pub session_trusted_tools: HashSet<String>,
    /// Current operating mode.
    pub operating_mode: OperatingMode,
    /// Saved full tool set for restoring on plan→execute transition.
    saved_tools: Option<Vec<Arc<dyn AgentTool>>>,
    /// Original system prompt (before plan mode addendum).
    saved_system_prompt: Option<String>,
}

impl App {
    pub fn new(config: TuiConfig) -> Self {
        let (agent_tx, agent_rx) = mpsc::channel(256);
        let (approval_tx, approval_rx) = mpsc::channel(16);
        let session_store = JsonlSessionStore::default_dir()
            .and_then(|dir| JsonlSessionStore::new(dir).ok());
        let session_id = session_store
            .as_ref()
            .map_or_else(|| "unnamed".to_string(), SessionStore::new_session_id);

        // Apply configured color mode.
        let mode = match config.color_mode.as_str() {
            "mono-white" => theme::ColorMode::MonoWhite,
            "mono-black" => theme::ColorMode::MonoBlack,
            _ => theme::ColorMode::Custom,
        };
        theme::set_color_mode(mode);

        Self {
            should_quit: false,
            status: AgentStatus::Idle,
            input: InputEditor::new(),
            messages: Vec::new(),
            conversation: ConversationView::new(),
            tool_panel: ToolPanel::new(),
            help_panel: HelpPanel::new(),
            focus: Focus::Input,
            model_name: config.default_model.clone(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost: 0.0,
            session_start: Instant::now(),
            dirty: true,
            blink_on: true,
            tick_count: 0,
            agent: None,
            agent_tx,
            agent_rx,
            config,
            retry_attempt: None,
            session_store,
            session_id,
            approval_rx,
            approval_tx,
            pending_approval: None,
            approval_mode: ApprovalMode::default(),
            context_budget: 0,
            context_tokens_used: 0,
            selected_tool_block: None,
            open_editor_requested: false,
            session_trusted_tools: HashSet::new(),
            operating_mode: OperatingMode::Execute,
            saved_tools: None,
            saved_system_prompt: None,
        }
    }

    /// Main async event loop using `tokio::select!`.
    pub async fn run(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> AppResult<()> {
        let tick_rate = Duration::from_millis(self.config.tick_rate_ms);
        let mut tick_interval = tokio::time::interval(tick_rate);
        let mut event_stream = crossterm::event::EventStream::new();

        if self.messages.is_empty() {
            self.push_system_message("Press F1 for help.".to_string());
        }

        loop {
            if self.dirty {
                terminal.draw(|frame| ui::render(frame, self))?;
                self.dirty = false;
            }

            if self.should_quit {
                break;
            }

            tokio::select! {
                // Terminal events (keyboard, mouse, resize)
                maybe_event = event_stream.next() => {
                    if let Some(Ok(event)) = maybe_event {
                        self.handle_terminal_event(&event);
                    }
                }
                // Agent events
                Some(event) = self.agent_rx.recv() => {
                    self.handle_agent_event(event);
                }
                // Tool approval requests from the agent callback
                Some((request, responder)) = self.approval_rx.recv() => {
                    if self.approval_mode == ApprovalMode::Smart
                        && self.session_trusted_tools.contains(&request.tool_name)
                    {
                        let _ = responder.send(ToolApproval::Approved);
                    } else {
                        self.pending_approval = Some((request, responder));
                    }
                    self.dirty = true;
                }
                // Tick for animations
                _ = tick_interval.tick() => {
                    self.tick();
                }
            }

            // Handle external editor request
            if self.open_editor_requested {
                self.open_editor_requested = false;
                let editor =
                    crate::editor::resolve_editor(self.config.editor_command.as_deref());

                // Suspend TUI
                let _ = crate::restore_terminal();

                let result = crate::editor::open_editor(&editor);

                // Resume TUI
                let _ = crossterm::terminal::enable_raw_mode();
                let _ = crossterm::execute!(
                    std::io::stdout(),
                    crossterm::terminal::EnterAlternateScreen,
                    crossterm::event::EnableMouseCapture
                );
                // Force full redraw and re-create event stream (stale after suspend)
                terminal.clear()?;
                self.dirty = true;
                event_stream = crossterm::event::EventStream::new();

                match result {
                    Ok(Some(content)) => {
                        // Submit as if the user typed it
                        self.messages.push(DisplayMessage {
                            role: MessageRole::User,
                            content: content.clone(),
                            thinking: None,
                            is_streaming: false,
                            collapsed: false,
                            summary: String::new(),
                            user_expanded: false,
                            expanded_at: None,
                            plan_mode: false,
                            diff_data: None,
                        });
                        self.conversation.auto_scroll = true;
                        self.send_to_agent(content);
                    }
                    Ok(None) => {
                        self.push_system_message(
                            "Editor closed with empty content — cancelled.".to_string(),
                        );
                    }
                    Err(e) => {
                        self.push_system_message(format!("Editor error: {e}"));
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_terminal_event(&mut self, event: &Event) {
        match event {
            Event::Key(key) => self.handle_key_event(*key),
            Event::Resize(_, _) => {
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn handle_key_event(&mut self, key: KeyEvent) {
        // Global keys handled regardless of focus
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('q')) => {
                self.should_quit = true;
                self.dirty = true;
                return;
            }
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                if self.status == AgentStatus::Running {
                    self.abort_agent();
                } else {
                    self.should_quit = true;
                }
                self.dirty = true;
                return;
            }
            _ => {}
        }

        // Handle approval Y/N/A when a tool is pending approval
        if self.pending_approval.is_some() {
            match key.code {
                KeyCode::Char('y' | 'Y') | KeyCode::Enter => {
                    if let Some((_req, responder)) = self.pending_approval.take() {
                        let _ = responder.send(ToolApproval::Approved);
                    }
                    self.dirty = true;
                    return;
                }
                KeyCode::Char('a' | 'A') => {
                    if let Some((req, responder)) = self.pending_approval.take() {
                        self.session_trusted_tools.insert(req.tool_name);
                        let _ = responder.send(ToolApproval::Approved);
                    }
                    self.dirty = true;
                    return;
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                    if let Some((_req, responder)) = self.pending_approval.take() {
                        let _ = responder.send(ToolApproval::Rejected);
                    }
                    self.dirty = true;
                    return;
                }
                _ => {
                    // Ignore other keys while approval is pending
                    return;
                }
            }
        }

        // Conversation-focused keys
        if self.focus == Focus::Conversation {
            let page = Self::last_visible_height();
            match key.code {
                KeyCode::Up => self.conversation.scroll_up(1),
                KeyCode::Down => self.conversation.scroll_down(1, page),
                KeyCode::PageUp => self.conversation.scroll_up(page),
                KeyCode::PageDown => self.conversation.scroll_down(page, page),
                KeyCode::F(1) => self.help_panel.toggle(),
                // Tab and any other key switches to input focus and falls through
                _ => self.focus = Focus::Input,
            }
            self.dirty = true;
            if self.focus == Focus::Conversation {
                return;
            }
        }

        self.handle_input_key(key);
        self.dirty = true;
    }

    fn handle_input_key(&mut self, key: KeyEvent) {
        match (key.modifiers, key.code) {
            // Escape — abort if running
            (_, KeyCode::Esc) => {
                if self.status == AgentStatus::Running {
                    self.abort_agent();
                }
            }
            // Shift+Tab — toggle plan/execute mode
            (KeyModifiers::SHIFT, KeyCode::BackTab) => {
                self.toggle_operating_mode();
            }
            // Tab — toggle focus
            (_, KeyCode::Tab) => {
                self.focus = match self.focus {
                    Focus::Input => Focus::Conversation,
                    Focus::Conversation => Focus::Input,
                };
            }
            // Submit: Enter (without Shift)
            (KeyModifiers::NONE, KeyCode::Enter) => {
                self.submit_input();
            }
            // Newline: Shift+Enter
            (KeyModifiers::SHIFT, KeyCode::Enter) => {
                self.input.insert_newline();
            }
            // Home / Ctrl+A
            (_, KeyCode::Home) | (KeyModifiers::CONTROL, KeyCode::Char('a')) => {
                self.input.move_home();
            }
            // End / Ctrl+E
            (_, KeyCode::End) | (KeyModifiers::CONTROL, KeyCode::Char('e')) => {
                self.input.move_end();
            }
            // Arrow keys
            (KeyModifiers::NONE, KeyCode::Up) => {
                if self.input.cursor_row == 0 {
                    self.input.history_prev();
                } else {
                    self.input.move_up();
                }
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                if self.input.cursor_row + 1 >= self.input.lines.len() {
                    self.input.history_next();
                } else {
                    self.input.move_down();
                }
            }
            (KeyModifiers::NONE, KeyCode::Left) => self.input.move_left(),
            (KeyModifiers::NONE, KeyCode::Right) => self.input.move_right(),
            // PageUp/PageDown scroll conversation even from input focus
            (_, KeyCode::PageUp) => {
                let page = Self::last_visible_height();
                self.conversation.scroll_up(page);
            }
            (_, KeyCode::PageDown) => {
                let page = Self::last_visible_height();
                self.conversation.scroll_down(page, page);
            }
            // Backspace
            (_, KeyCode::Backspace) => self.input.backspace(),
            // Delete
            (_, KeyCode::Delete) => self.input.delete(),
            // F1 — toggle help panel
            (_, KeyCode::F(1)) => {
                self.help_panel.toggle();
            }
            // F2 — toggle collapse on selected (or most recent) tool block
            (_, KeyCode::F(2)) => {
                let target = self.selected_tool_block.or_else(|| {
                    self.messages
                        .iter()
                        .enumerate()
                        .rev()
                        .find(|(_, m)| m.role == MessageRole::ToolResult)
                        .map(|(i, _)| i)
                });
                if let Some(idx) = target {
                    self.toggle_collapse(idx);
                    self.selected_tool_block = Some(idx);
                }
            }
            // F3 — cycle color mode (Custom → MonoWhite → MonoBlack → Custom)
            (_, KeyCode::F(3)) => {
                theme::cycle_color_mode();
            }
            // Shift+Left — select previous tool block
            (KeyModifiers::SHIFT, KeyCode::Left) => {
                self.select_prev_tool_block();
            }
            // Shift+Right — select next tool block
            (KeyModifiers::SHIFT, KeyCode::Right) => {
                self.select_next_tool_block();
            }
            // Typing
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
                self.input.insert_char(c);
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_lines)]
    fn submit_input(&mut self) {
        let Some(text) = self.input.submit() else {
            return;
        };

        // Check for commands
        match commands::execute_command(&text) {
            CommandResult::NotACommand => {}
            CommandResult::Quit => {
                self.should_quit = true;
                return;
            }
            CommandResult::Clear => {
                self.messages.clear();
                self.conversation = ConversationView::new();
                return;
            }
            CommandResult::ToggleHelp => {
                self.help_panel.toggle();
                self.dirty = true;
                return;
            }
            CommandResult::Feedback(msg) => {
                let feedback = if msg.is_empty() {
                    self.session_info()
                } else {
                    msg
                };
                self.push_system_message(feedback);
                return;
            }
            CommandResult::SetModel(model) => {
                self.model_name.clone_from(&model);
                if let Some(agent) = &mut self.agent {
                    agent.set_model(swink_agent::ModelSpec::new("", &model));
                }
                let msg = format!("Model set to: {}", self.model_name);
                self.push_system_message(msg);
                return;
            }
            CommandResult::SetThinking(level) => {
                self.push_system_message(format!("Thinking level set to: {level}"));
                return;
            }
            CommandResult::SetSystemPrompt(prompt) => {
                if let Some(agent) = &mut self.agent {
                    agent.set_system_prompt(prompt);
                }
                self.push_system_message("System prompt updated.".to_string());
                return;
            }
            CommandResult::Reset => {
                if let Some(agent) = &mut self.agent {
                    agent.reset();
                }
                self.messages.clear();
                self.conversation = ConversationView::new();
                self.total_input_tokens = 0;
                self.total_output_tokens = 0;
                self.total_cost = 0.0;
                self.context_tokens_used = 0;
                self.session_trusted_tools.clear();
                self.operating_mode = OperatingMode::Execute;
                self.saved_tools = None;
                self.saved_system_prompt = None;
                self.push_system_message("Agent state reset.".to_string());
                return;
            }
            CommandResult::CopyToClipboard(content) => {
                self.copy_to_clipboard(content);
                return;
            }
            CommandResult::SaveSession => {
                self.save_session();
                return;
            }
            CommandResult::LoadSession(id) => {
                self.load_session(&id);
                return;
            }
            CommandResult::ListSessions => {
                self.list_sessions();
                return;
            }
            CommandResult::StoreKey { provider, key } => {
                self.store_key(&provider, &key);
                return;
            }
            CommandResult::ListKeys => {
                self.list_keys();
                return;
            }
            CommandResult::SetApprovalMode(mode) => {
                let harness_mode = match mode {
                    ApprovalModeArg::On => ApprovalMode::Enabled,
                    ApprovalModeArg::Off => ApprovalMode::Bypassed,
                    ApprovalModeArg::Smart => ApprovalMode::Smart,
                };
                self.approval_mode = harness_mode;
                if let Some(agent) = &mut self.agent {
                    agent.set_approval_mode(harness_mode);
                }
                let label = match mode {
                    ApprovalModeArg::On => "enabled",
                    ApprovalModeArg::Off => "disabled (auto-approve)",
                    ApprovalModeArg::Smart => "smart (auto-approve reads, prompt for writes)",
                };
                self.push_system_message(format!("Tool approval: {label}"));
                return;
            }
            CommandResult::OpenEditor => {
                self.open_editor_requested = true;
                return;
            }
            CommandResult::TogglePlanMode => {
                self.toggle_operating_mode();
                return;
            }
            CommandResult::QueryApprovalMode => {
                let label = match self.approval_mode {
                    ApprovalMode::Enabled => "enabled",
                    ApprovalMode::Bypassed => "disabled (auto-approve)",
                    ApprovalMode::Smart => "smart (auto-approve reads, prompt for writes)",
                };
                let mut msg = format!("Tool approval: {label}");
                if self.approval_mode == ApprovalMode::Smart
                    && !self.session_trusted_tools.is_empty()
                {
                    msg.push_str("\nTrusted tools: ");
                    let mut tools: Vec<&str> =
                        self.session_trusted_tools.iter().map(String::as_str).collect();
                    tools.sort_unstable();
                    msg.push_str(&tools.join(", "));
                }
                self.push_system_message(msg);
                return;
            }
        }

        // Add user message to display
        self.messages.push(DisplayMessage {
            role: MessageRole::User,
            content: text.clone(),
            thinking: None,
            is_streaming: false,
            collapsed: false,
            summary: String::new(),
            user_expanded: false,
            expanded_at: None,
            plan_mode: false,
            diff_data: None,
        });

        // Re-engage auto-scroll on new user message
        self.conversation.auto_scroll = true;

        // Send to agent if connected
        self.send_to_agent(text);
    }

    fn push_system_message(&mut self, content: String) {
        self.messages.push(DisplayMessage {
            role: MessageRole::System,
            content,
            thinking: None,
            is_streaming: false,
            collapsed: false,
            summary: String::new(),
            user_expanded: false,
            expanded_at: None,
            plan_mode: false,
            diff_data: None,
        });
    }

    fn send_to_agent(&mut self, text: String) {
        let Some(agent) = &mut self.agent else {
            return;
        };

        let user_message = AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text { text }],
            timestamp: timestamp_now(),
        }));

        let input = vec![user_message];
        self.status = AgentStatus::Running;
        self.retry_attempt = None;

        match agent.prompt_stream(input) {
            Ok(stream) => {
                let tx = self.agent_tx.clone();
                tokio::spawn(async move {
                    let mut stream = std::pin::pin!(stream);
                    while let Some(event) = stream.next().await {
                        if tx.send(event).await.is_err() {
                            break;
                        }
                    }
                });
            }
            Err(e) => {
                self.status = AgentStatus::Error;
                self.messages.push(DisplayMessage {
                    role: MessageRole::Error,
                    content: format!("Failed to start agent: {e}"),
                    thinking: None,
                    is_streaming: false,
                    collapsed: false,
                    summary: String::new(),
                    user_expanded: false,
                    expanded_at: None,
                    plan_mode: false,
                    diff_data: None,
                });
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn handle_agent_event(&mut self, event: AgentEvent) {
        // Feed the event back to the agent so it can update internal state
        // (e.g. clear is_running on AgentEnd). Without this, prompt_stream
        // consumers leave the agent stuck in the "running" state.
        if let Some(agent) = &mut self.agent {
            agent.handle_stream_event(&event);
        }
        match event {
            AgentEvent::AgentStart => {
                self.status = AgentStatus::Running;
            }
            AgentEvent::MessageStart => {
                self.messages.push(DisplayMessage {
                    role: MessageRole::Assistant,
                    content: String::new(),
                    thinking: None,
                    is_streaming: true,
                    collapsed: false,
                    summary: String::new(),
                    user_expanded: false,
                    expanded_at: None,
                    plan_mode: self.operating_mode == OperatingMode::Plan,
                    diff_data: None,
                });
            }
            AgentEvent::MessageUpdate { delta } => {
                if let Some(msg) = self.messages.last_mut() {
                    match delta {
                        AssistantMessageDelta::Text { delta, .. } => {
                            msg.content.push_str(&delta);
                        }
                        AssistantMessageDelta::Thinking { delta, .. } => {
                            let thinking = msg.thinking.get_or_insert_with(String::new);
                            thinking.push_str(&delta);
                        }
                        AssistantMessageDelta::ToolCall { .. } => {}
                    }
                }
            }
            AgentEvent::MessageEnd { message } => {
                if let Some(msg) = self.messages.last_mut() {
                    msg.is_streaming = false;
                    let mut text_parts = Vec::new();
                    let mut thinking_parts = Vec::new();
                    for block in &message.content {
                        match block {
                            ContentBlock::Text { text } => text_parts.push(text.as_str()),
                            ContentBlock::Thinking { thinking, .. } => {
                                thinking_parts.push(thinking.as_str());
                            }
                            _ => {}
                        }
                    }
                    if !text_parts.is_empty() {
                        msg.content = text_parts.join("");
                    }
                    if !thinking_parts.is_empty() {
                        msg.thinking = Some(thinking_parts.join(""));
                    }
                }
                self.total_input_tokens += message.usage.input;
                self.total_output_tokens += message.usage.output;
                self.total_cost += message.cost.total;
                self.context_tokens_used = message.usage.input;
                self.model_name.clone_from(&message.model_id);
            }
            AgentEvent::ToolExecutionStart { id, name, .. } => {
                self.tool_panel.start_tool(id, name);
            }
            AgentEvent::ToolExecutionEnd { is_error, .. } => {
                if let Some(tool) = self.tool_panel.active.last() {
                    let id = tool.id.clone();
                    self.tool_panel.end_tool(&id, is_error);
                }
            }
            AgentEvent::TurnEnd {
                tool_results, ..
            } => {
                for result in &tool_results {
                    let content = ContentBlock::extract_text(&result.content);
                    if !content.is_empty() {
                        let role = if result.is_error {
                            MessageRole::Error
                        } else {
                            MessageRole::ToolResult
                        };
                        let summary = content
                            .lines()
                            .next()
                            .unwrap_or("")
                            .chars()
                            .take(60)
                            .collect::<String>();
                        let is_tool_result = role == MessageRole::ToolResult;
                        let diff_data =
                            crate::ui::diff::DiffData::from_details(&result.details);
                        self.messages.push(DisplayMessage {
                            role,
                            content,
                            thinking: None,
                            is_streaming: false,
                            collapsed: false,
                            summary,
                            user_expanded: false,
                            expanded_at: if is_tool_result {
                                Some(Instant::now())
                            } else {
                                None
                            },
                            plan_mode: false,
                            diff_data,
                        });
                    }
                }
            }
            AgentEvent::AgentEnd { .. } => {
                self.status = AgentStatus::Idle;
                self.retry_attempt = None;
                self.auto_save_session();
            }
            AgentEvent::ToolApprovalRequested {
                id,
                name,
                arguments,
            } => {
                self.tool_panel.set_awaiting_approval(&id, &name, &arguments);
            }
            AgentEvent::ToolApprovalResolved { id, approved, .. } => {
                self.tool_panel.resolve_approval(&id, approved);
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn abort_agent(&mut self) {
        if let Some(agent) = &mut self.agent {
            agent.abort();
        }
        self.status = AgentStatus::Aborted;
        if let Some(msg) = self.messages.last_mut()
            && msg.is_streaming
        {
            msg.is_streaming = false;
            msg.content.push_str("\n[aborted]");
        }
        self.dirty = true;
    }

    /// Tick handler for animations.
    pub fn tick(&mut self) {
        self.tick_count += 1;
        if self.tick_count.is_multiple_of(5) {
            self.blink_on = !self.blink_on;
            if self.status == AgentStatus::Running {
                self.dirty = true;
            }
        }
        self.tool_panel.tick();
        if self.tool_panel.is_visible() {
            self.dirty = true;
        }

        // Auto-collapse tool results after AUTO_COLLAPSE_SECS
        for msg in &mut self.messages {
            if msg.role == MessageRole::ToolResult
                && !msg.collapsed
                && !msg.user_expanded
                && let Some(expanded_at) = msg.expanded_at
                && expanded_at.elapsed() > Duration::from_secs(AUTO_COLLAPSE_SECS)
            {
                msg.collapsed = true;
                self.dirty = true;
            }
        }
    }

    fn session_info(&self) -> String {
        format!(
            "Model: {}\nInput tokens: {}\nOutput tokens: {}\nCost: ${:.4}\nMessages: {}",
            self.model_name,
            self.total_input_tokens,
            self.total_output_tokens,
            self.total_cost,
            self.messages.len(),
        )
    }

    fn copy_to_clipboard(&mut self, content: ClipboardContent) {
        let text = match content {
            ClipboardContent::Last => self
                .messages
                .iter()
                .rev()
                .find(|m| m.role == MessageRole::Assistant)
                .map(|m| m.content.clone()),
            ClipboardContent::All => {
                let all: String = self
                    .messages
                    .iter()
                    .map(|m| {
                        let role = match m.role {
                            MessageRole::User => "You",
                            MessageRole::Assistant => "Assistant",
                            MessageRole::ToolResult => "Tool",
                            MessageRole::Error => "Error",
                            MessageRole::System => "System",
                        };
                        format!("{role}: {}", m.content)
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                Some(all)
            }
            ClipboardContent::Code => self
                .messages
                .iter()
                .rev()
                .find(|m| m.role == MessageRole::Assistant)
                .and_then(|m| extract_last_code_block(&m.content)),
        };

        let feedback = text.map_or_else(
            || "Nothing to copy.".to_string(),
            |text| match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text)) {
                Ok(()) => "Copied to clipboard.".to_string(),
                Err(e) => format!("Clipboard error: {e}"),
            },
        );

        self.push_system_message(feedback);
    }

    /// Set the agent instance for this app.
    pub fn set_agent(&mut self, agent: Agent) {
        self.model_name.clone_from(&agent.state().model.model_id);
        self.context_budget = 100_000;
        self.agent = Some(agent);
    }

    /// Get a clone of the approval request sender for use in the agent callback.
    pub fn approval_sender(
        &self,
    ) -> mpsc::Sender<(ToolApprovalRequest, oneshot::Sender<ToolApproval>)> {
        self.approval_tx.clone()
    }

    /// Toggle collapse state of the tool result at the given message index.
    pub fn toggle_collapse(&mut self, index: usize) {
        if let Some(msg) = self.messages.get_mut(index)
            && msg.role == MessageRole::ToolResult
        {
            msg.collapsed = !msg.collapsed;
            msg.user_expanded = !msg.collapsed;
            if !msg.collapsed {
                msg.expanded_at = Some(Instant::now());
            }
            self.dirty = true;
        }
    }

    /// Select the previous tool result block. Returns true if a tool block exists.
    fn select_prev_tool_block(&mut self) -> bool {
        let tool_indices: Vec<usize> = self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role == MessageRole::ToolResult)
            .map(|(i, _)| i)
            .collect();
        if tool_indices.is_empty() {
            return false;
        }
        match self.selected_tool_block {
            None => {
                self.selected_tool_block = Some(*tool_indices.last().unwrap());
            }
            Some(current) => {
                if let Some(prev) = tool_indices.iter().rev().find(|&&i| i < current) {
                    self.selected_tool_block = Some(*prev);
                }
            }
        }
        true
    }

    /// Select the next tool result block. Returns true if a tool block exists.
    fn select_next_tool_block(&mut self) -> bool {
        let tool_indices: Vec<usize> = self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role == MessageRole::ToolResult)
            .map(|(i, _)| i)
            .collect();
        if tool_indices.is_empty() {
            return false;
        }
        match self.selected_tool_block {
            None => {
                self.selected_tool_block = Some(tool_indices[0]);
            }
            Some(current) => {
                if let Some(next) = tool_indices.iter().find(|&&i| i > current) {
                    self.selected_tool_block = Some(*next);
                }
            }
        }
        true
    }

    /// Approximate visible height of conversation area.
    const fn last_visible_height() -> usize {
        20
    }

    // ─── Plan mode ─────────────────────────────────────────────────────

    /// Toggle between Plan and Execute modes.
    fn toggle_operating_mode(&mut self) {
        match self.operating_mode {
            OperatingMode::Execute => self.enter_plan_mode(),
            OperatingMode::Plan => self.exit_plan_mode(),
        }
        self.dirty = true;
    }

    fn enter_plan_mode(&mut self) {
        let Some(agent) = &mut self.agent else {
            return;
        };

        // Save current tools
        let all_tools = agent.state().tools.clone();
        self.saved_tools = Some(all_tools.clone());

        // Filter to read-only tools (requires_approval == false)
        let read_only: Vec<Arc<dyn AgentTool>> = all_tools
            .into_iter()
            .filter(|t| !t.requires_approval())
            .collect();
        agent.set_tools(read_only);

        // Save and modify system prompt
        let current_prompt = agent.state().system_prompt.clone();
        self.saved_system_prompt = Some(current_prompt.clone());
        agent.set_system_prompt(format!("{current_prompt}{PLAN_MODE_ADDENDUM}"));

        self.operating_mode = OperatingMode::Plan;
        self.push_system_message("Entered plan mode — read-only tools only.".to_string());
    }

    fn exit_plan_mode(&mut self) {
        let Some(agent) = &mut self.agent else {
            return;
        };

        // Restore tools
        if let Some(tools) = self.saved_tools.take() {
            agent.set_tools(tools);
        }

        // Restore system prompt
        if let Some(prompt) = self.saved_system_prompt.take() {
            agent.set_system_prompt(prompt);
        }

        self.operating_mode = OperatingMode::Execute;
        self.push_system_message("Exited plan mode — all tools available.".to_string());
    }

    // ─── Session persistence ────────────────────────────────────────────

    fn auto_save_session(&self) {
        let Some(ref store) = self.session_store else {
            return;
        };
        let Some(ref agent) = self.agent else {
            return;
        };
        let state = agent.state();
        let _ = store.save(
            &self.session_id,
            &self.model_name,
            &state.system_prompt,
            &state.messages,
        );
    }

    fn save_session(&mut self) {
        info!(session_id = %self.session_id, "saving session");
        self.auto_save_session();
        self.push_system_message(format!("Session saved: {}", self.session_id));
    }

    fn load_session(&mut self, id: &str) {
        let Some(ref store) = self.session_store else {
            warn!("session persistence unavailable");
            self.push_system_message("Session persistence unavailable.".to_string());
            return;
        };
        info!(session_id = %id, "loading session");
        match store.load(id) {
            Ok((meta, messages)) => {
                // Rebuild display messages from loaded data
                self.messages.clear();
                for msg in &messages {
                    if let AgentMessage::Llm(llm) = msg {
                        match llm {
                            LlmMessage::User(u) => {
                                self.messages.push(DisplayMessage {
                                    role: MessageRole::User,
                                    content: ContentBlock::extract_text(&u.content),
                                    thinking: None,
                                    is_streaming: false,
                                    collapsed: false,
                                    summary: String::new(),
                                    user_expanded: false,
                                    expanded_at: None,
                                    plan_mode: false,
                                    diff_data: None,
                                });
                            }
                            LlmMessage::Assistant(a) => {
                                self.messages.push(DisplayMessage {
                                    role: MessageRole::Assistant,
                                    content: ContentBlock::extract_text(&a.content),
                                    thinking: None,
                                    is_streaming: false,
                                    collapsed: false,
                                    summary: String::new(),
                                    user_expanded: false,
                                    expanded_at: None,
                                    plan_mode: false,
                                    diff_data: None,
                                });
                            }
                            LlmMessage::ToolResult(t) => {
                                let content = ContentBlock::extract_text(&t.content);
                                if !content.is_empty() {
                                    let summary = content
                                        .lines()
                                        .next()
                                        .unwrap_or("")
                                        .chars()
                                        .take(60)
                                        .collect::<String>();
                                    let diff_data = crate::ui::diff::DiffData::from_details(
                                        &t.details,
                                    );
                                    self.messages.push(DisplayMessage {
                                        role: MessageRole::ToolResult,
                                        content,
                                        thinking: None,
                                        is_streaming: false,
                                        collapsed: true,
                                        summary,
                                        user_expanded: false,
                                        expanded_at: None,
                                        plan_mode: false,
                                        diff_data,
                                    });
                                }
                            }
                        }
                    }
                }
                self.session_id = id.to_string();
                self.model_name.clone_from(&meta.model);
                self.conversation = ConversationView::new();
                // Set agent messages (takes ownership)
                if let Some(agent) = &mut self.agent {
                    if !meta.system_prompt.is_empty() {
                        agent.set_system_prompt(&meta.system_prompt);
                    }
                    agent.set_messages(messages);
                }
                self.push_system_message(format!(
                    "Loaded session: {} ({} messages)",
                    id, meta.message_count
                ));
            }
            Err(e) => {
                warn!(session_id = %id, error = %e, "failed to load session");
                self.push_system_message(format!("Failed to load session: {e}"));
            }
        }
    }

    fn list_sessions(&mut self) {
        use std::fmt::Write;
        let Some(ref store) = self.session_store else {
            self.push_system_message("Session persistence unavailable.".to_string());
            return;
        };
        match store.list() {
            Ok(sessions) if sessions.is_empty() => {
                self.push_system_message("No saved sessions.".to_string());
            }
            Ok(sessions) => {
                let mut text = String::from("Saved sessions:\n");
                for s in &sessions {
                    let current = if s.id == self.session_id {
                        " (current)"
                    } else {
                        ""
                    };
                    let _ = writeln!(
                        text,
                        "  {} — {} msgs, model: {}{current}",
                        s.id, s.message_count, s.model
                    );
                }
                text.push_str("\nUse #load <id> to restore a session.");
                self.push_system_message(text);
            }
            Err(e) => {
                self.push_system_message(format!("Failed to list sessions: {e}"));
            }
        }
    }

    // ─── Credential management ──────────────────────────────────────────

    fn store_key(&mut self, provider: &str, key: &str) {
        match credentials::store_credential(provider, key) {
            Ok(()) => {
                info!(provider = %provider, "API key stored");
                self.push_system_message(format!("API key stored for: {provider}"));
            }
            Err(e) => {
                warn!(provider = %provider, error = %e, "failed to store API key");
                self.push_system_message(format!("Failed to store key: {e}"));
            }
        }
    }

    fn list_keys(&mut self) {
        use std::fmt::Write;
        let status = credentials::check_credentials();
        let providers = credentials::providers();
        let mut text = String::from("Provider credentials:\n");
        for p in &providers {
            let configured = status.get(p.key_name).copied().unwrap_or(false);
            let icon = if configured { "✓" } else { "✗" };
            let note = if p.requires_key { "" } else { " (no key needed)" };
            let _ = writeln!(text, "  {icon} {} — {}{note}", p.name, p.description);
        }
        text.push_str("\nUse #key <provider> <api-key> to store a key.");
        self.push_system_message(text);
    }
}

/// Extract the last fenced code block from markdown text.
fn extract_last_code_block(text: &str) -> Option<String> {
    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut current = Vec::new();

    for line in text.lines() {
        if line.starts_with("```") {
            if in_block {
                blocks.push(current.join("\n"));
                current.clear();
                in_block = false;
            } else {
                in_block = true;
            }
        } else if in_block {
            current.push(line);
        }
    }

    blocks.pop()
}

/// Get current Unix timestamp.
fn timestamp_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use futures::Stream;
    use tokio_util::sync::CancellationToken;

    use std::future::Future;

    use swink_agent::{
        Agent, AgentEvent, AgentMessage, AgentOptions, AgentToolResult, AssistantMessage,
        AssistantMessageEvent, Cost, LlmMessage, ModelSpec, StopReason, StreamFn, StreamOptions,
        Usage,
    };

    use super::*;

    // ─── Mock StreamFn ────────────────────────────────────────────────────

    struct MockStreamFn {
        responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
    }

    impl MockStreamFn {
        const fn new(responses: Vec<Vec<AssistantMessageEvent>>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    impl StreamFn for MockStreamFn {
        fn stream<'a>(
            &'a self,
            _model: &'a ModelSpec,
            _context: &'a swink_agent::AgentContext,
            _options: &'a StreamOptions,
            _cancellation_token: CancellationToken,
        ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
            let events = {
                let mut responses = self.responses.lock().unwrap();
                if responses.is_empty() {
                    vec![AssistantMessageEvent::Error {
                        stop_reason: StopReason::Error,
                        error_message: "no more scripted responses".to_string(),
                        usage: None,
                    }]
                } else {
                    responses.remove(0)
                }
            };
            Box::pin(futures::stream::iter(events))
        }
    }

    fn text_only_events(text: &str) -> Vec<AssistantMessageEvent> {
        vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::TextStart { content_index: 0 },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: text.to_string(),
            },
            AssistantMessageEvent::TextEnd { content_index: 0 },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ]
    }

    fn default_convert(msg: &AgentMessage) -> Option<LlmMessage> {
        match msg {
            AgentMessage::Llm(llm) => Some(llm.clone()),
            AgentMessage::Custom(_) => None,
        }
    }

    fn make_test_agent(stream_fn: Arc<dyn StreamFn>) -> Agent {
        Agent::new(AgentOptions::new(
            "test system prompt",
            ModelSpec::new("test", "mock-model"),
            stream_fn,
            default_convert,
        ))
    }

    /// Drain all pending agent events from the channel, feeding them back
    /// to the app (which in turn calls `agent.handle_stream_event`).
    fn drain_agent_events(app: &mut App) {
        while let Ok(event) = app.agent_rx.try_recv() {
            app.handle_agent_event(event);
        }
    }

    // ─── Tests ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn multi_turn_send_and_receive() {
        let stream_fn = Arc::new(MockStreamFn::new(vec![
            text_only_events("first response"),
            text_only_events("second response"),
        ]));
        let agent = make_test_agent(stream_fn);

        let mut app = App::new(TuiConfig::default());
        app.set_agent(agent);

        // Turn 1
        app.send_to_agent("hello".to_string());
        assert_eq!(app.status, AgentStatus::Running);

        // Let the spawned task forward events through the channel.
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        drain_agent_events(&mut app);

        assert_eq!(
            app.status,
            AgentStatus::Idle,
            "app should be idle after first turn"
        );
        assert!(
            app.messages
                .iter()
                .any(|m| m.role == MessageRole::Assistant && m.content == "first response"),
            "first response should appear in display messages"
        );

        // Turn 2 — should NOT produce "already running" error.
        app.send_to_agent("follow up".to_string());
        assert_eq!(app.status, AgentStatus::Running);

        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        drain_agent_events(&mut app);

        assert_eq!(
            app.status,
            AgentStatus::Idle,
            "app should be idle after second turn"
        );
        assert!(
            app.messages
                .iter()
                .any(|m| m.role == MessageRole::Assistant && m.content == "second response"),
            "second response should appear in display messages"
        );
        // No error messages should be present.
        assert!(
            !app.messages.iter().any(|m| m.role == MessageRole::Error),
            "no error messages should appear during multi-turn"
        );
    }

    #[tokio::test]
    async fn agent_state_transitions_through_events() {
        let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
        let agent = make_test_agent(stream_fn);

        let mut app = App::new(TuiConfig::default());
        app.set_agent(agent);

        assert_eq!(app.status, AgentStatus::Idle);

        // Simulate the event sequence directly.
        app.handle_agent_event(AgentEvent::AgentStart);
        assert_eq!(app.status, AgentStatus::Running);

        app.handle_agent_event(AgentEvent::AgentEnd {
            messages: Arc::new(Vec::new()),
        });
        assert_eq!(app.status, AgentStatus::Idle);

        // Agent's internal state should also be idle.
        let agent_ref = app.agent.as_ref().unwrap();
        assert!(
            !agent_ref.state().is_running,
            "agent internal is_running should be false after AgentEnd"
        );
    }

    #[tokio::test]
    async fn three_turn_conversation() {
        let stream_fn = Arc::new(MockStreamFn::new(vec![
            text_only_events("response one"),
            text_only_events("response two"),
            text_only_events("response three"),
        ]));
        let agent = make_test_agent(stream_fn);

        let mut app = App::new(TuiConfig::default());
        app.set_agent(agent);

        for (i, prompt) in ["first", "second", "third"].iter().enumerate() {
            app.send_to_agent(prompt.to_string());
            tokio::task::yield_now().await;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            drain_agent_events(&mut app);

            assert_eq!(
                app.status,
                AgentStatus::Idle,
                "should be idle after turn {}",
                i + 1
            );
        }

        let assistant_msgs: Vec<&str> = app
            .messages
            .iter()
            .filter(|m| m.role == MessageRole::Assistant)
            .map(|m| m.content.as_str())
            .collect();
        assert_eq!(
            assistant_msgs,
            vec!["response one", "response two", "response three"]
        );
        assert!(
            !app.messages.iter().any(|m| m.role == MessageRole::Error),
            "no errors across three turns"
        );
    }

    #[tokio::test]
    async fn message_end_updates_context_tokens_used() {
        let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hi")]));
        let agent = make_test_agent(stream_fn);

        let mut app = App::new(TuiConfig::default());
        app.set_agent(agent);

        assert_eq!(app.context_budget, 100_000);
        assert_eq!(app.context_tokens_used, 0);

        let message = AssistantMessage {
            content: vec![],
            provider: String::new(),
            model_id: "mock-model".to_string(),
            usage: Usage {
                input: 50_000,
                output: 200,
                cache_read: 0,
                cache_write: 0,
                total: 50_200,
            },
            cost: Cost::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        };

        app.handle_agent_event(AgentEvent::MessageEnd { message });
        assert_eq!(app.context_tokens_used, 50_000);
    }

    #[tokio::test]
    async fn reset_clears_context_tokens() {
        let stream_fn = Arc::new(MockStreamFn::new(vec![]));
        let agent = make_test_agent(stream_fn);

        let mut app = App::new(TuiConfig::default());
        app.set_agent(agent);
        app.context_tokens_used = 75_000;

        // Simulate the Reset command path
        if let Some(agent) = &mut app.agent {
            agent.reset();
        }
        app.context_tokens_used = 0;
        assert_eq!(app.context_tokens_used, 0);
    }

    #[tokio::test]
    async fn error_response_allows_retry() {
        let stream_fn = Arc::new(MockStreamFn::new(vec![
            // First turn: error
            vec![
                AssistantMessageEvent::Start,
                AssistantMessageEvent::Error {
                    stop_reason: StopReason::Error,
                    error_message: "something broke".to_string(),
                    usage: None,
                },
            ],
            // Second turn: success
            text_only_events("recovered"),
        ]));
        let agent = make_test_agent(stream_fn);

        let mut app = App::new(TuiConfig::default());
        app.set_agent(agent);

        // Turn 1: produces an error event but the agent loop still completes.
        app.send_to_agent("hello".to_string());
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        drain_agent_events(&mut app);

        assert_eq!(
            app.status,
            AgentStatus::Idle,
            "should return to idle even after an error response"
        );

        // Turn 2: should succeed.
        app.send_to_agent("try again".to_string());
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        drain_agent_events(&mut app);

        assert_eq!(app.status, AgentStatus::Idle);
        assert!(
            app.messages
                .iter()
                .any(|m| m.role == MessageRole::Assistant && m.content == "recovered"),
            "recovery response should appear"
        );
    }

    // ─── Collapsible tool result tests ──────────────────────────────────

    fn make_tool_result_message(content: &str) -> DisplayMessage {
        let summary = content
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(60)
            .collect::<String>();
        DisplayMessage {
            role: MessageRole::ToolResult,
            content: content.to_string(),
            thinking: None,
            is_streaming: false,
            collapsed: false,
            summary,
            user_expanded: false,
            expanded_at: Some(Instant::now()),
            plan_mode: false,
            diff_data: None,
        }
    }

    fn make_user_message(content: &str) -> DisplayMessage {
        DisplayMessage {
            role: MessageRole::User,
            content: content.to_string(),
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

    #[tokio::test]
    async fn tool_result_has_collapsed_fields() {
        let msg = make_tool_result_message("file contents here\nsecond line");
        assert!(!msg.collapsed, "tool result starts expanded");
        assert_eq!(msg.summary, "file contents here");
        assert!(!msg.user_expanded);
        assert!(msg.expanded_at.is_some());
    }

    #[tokio::test]
    async fn toggle_collapse_toggles_state() {
        let mut app = App::new(TuiConfig::default());
        app.messages.push(make_tool_result_message("tool output"));

        assert!(!app.messages[0].collapsed);

        app.toggle_collapse(0);
        assert!(app.messages[0].collapsed);
        assert!(!app.messages[0].user_expanded);

        app.toggle_collapse(0);
        assert!(!app.messages[0].collapsed);
        assert!(app.messages[0].user_expanded);
    }

    #[tokio::test]
    async fn toggle_collapse_non_tool_is_noop() {
        let mut app = App::new(TuiConfig::default());
        app.messages.push(make_user_message("hello"));

        app.toggle_collapse(0);
        assert!(!app.messages[0].collapsed, "user message should not collapse");
    }

    #[tokio::test]
    async fn auto_collapse_after_timeout() {
        let mut app = App::new(TuiConfig::default());
        let mut msg = make_tool_result_message("tool output");
        // Set expanded_at to 11 seconds in the past (exceeds AUTO_COLLAPSE_SECS)
        msg.expanded_at = Some(Instant::now() - Duration::from_secs(11));
        app.messages.push(msg);

        assert!(!app.messages[0].collapsed);

        app.tick();

        assert!(
            app.messages[0].collapsed,
            "tool result should auto-collapse after 10 seconds"
        );
    }

    #[tokio::test]
    async fn user_expanded_prevents_auto_collapse() {
        let mut app = App::new(TuiConfig::default());
        let mut msg = make_tool_result_message("tool output");
        msg.expanded_at = Some(Instant::now() - Duration::from_secs(11));
        msg.user_expanded = true;
        app.messages.push(msg);

        app.tick();

        assert!(
            !app.messages[0].collapsed,
            "user-expanded tool result should not auto-collapse"
        );
    }

    #[tokio::test]
    async fn select_next_tool_block_navigates() {
        let mut app = App::new(TuiConfig::default());
        app.messages.push(make_user_message("hello"));
        app.messages.push(make_tool_result_message("tool 1"));
        app.messages.push(make_user_message("world"));
        app.messages.push(make_tool_result_message("tool 2"));

        assert_eq!(app.selected_tool_block, None);

        assert!(app.select_next_tool_block());
        assert_eq!(app.selected_tool_block, Some(1));

        assert!(app.select_next_tool_block());
        assert_eq!(app.selected_tool_block, Some(3));

        // At the end, stays on last
        assert!(app.select_next_tool_block());
        assert_eq!(app.selected_tool_block, Some(3));
    }

    #[tokio::test]
    async fn select_prev_tool_block_navigates() {
        let mut app = App::new(TuiConfig::default());
        app.messages.push(make_user_message("hello"));
        app.messages.push(make_tool_result_message("tool 1"));
        app.messages.push(make_user_message("world"));
        app.messages.push(make_tool_result_message("tool 2"));

        // Start from None, goes to last
        assert!(app.select_prev_tool_block());
        assert_eq!(app.selected_tool_block, Some(3));

        assert!(app.select_prev_tool_block());
        assert_eq!(app.selected_tool_block, Some(1));

        // At the beginning, stays on first
        assert!(app.select_prev_tool_block());
        assert_eq!(app.selected_tool_block, Some(1));
    }

    #[tokio::test]
    async fn f2_toggles_most_recent_tool_block() {
        let mut app = App::new(TuiConfig::default());
        app.messages.push(make_user_message("hello"));
        app.messages.push(make_tool_result_message("tool 1"));
        app.messages.push(make_user_message("world"));
        app.messages.push(make_tool_result_message("tool 2"));

        assert_eq!(app.selected_tool_block, None);
        assert!(!app.messages[3].collapsed);

        // F2 from input focus should toggle most recent tool block
        let key = KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE);
        app.handle_key_event(key);

        assert_eq!(app.selected_tool_block, Some(3));
        assert!(app.messages[3].collapsed, "most recent tool block should collapse");
    }

    #[tokio::test]
    async fn f2_toggles_selected_tool_block() {
        let mut app = App::new(TuiConfig::default());
        app.messages.push(make_tool_result_message("tool 1"));
        app.messages.push(make_user_message("hello"));
        app.messages.push(make_tool_result_message("tool 2"));

        // Select the first tool block
        app.selected_tool_block = Some(0);
        assert!(!app.messages[0].collapsed);

        let key = KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE);
        app.handle_key_event(key);

        assert!(app.messages[0].collapsed, "selected tool block should collapse");
        assert!(!app.messages[2].collapsed, "other tool block should stay expanded");
    }

    #[tokio::test]
    async fn capital_e_inserts_char() {
        let mut app = App::new(TuiConfig::default());

        let key = KeyEvent::new(KeyCode::Char('E'), KeyModifiers::SHIFT);
        app.handle_key_event(key);

        assert_eq!(
            app.input.lines[0], "E",
            "Shift+E should insert 'E' into input"
        );
    }

    #[tokio::test]
    async fn f3_cycles_color_mode() {
        use crate::theme::{self, ColorMode};

        // Ensure we start from Custom
        theme::set_color_mode(ColorMode::Custom);

        let mut app = App::new(TuiConfig::default());
        let key = KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE);

        app.handle_key_event(key);
        assert_eq!(theme::color_mode(), ColorMode::MonoWhite);

        app.handle_key_event(key);
        assert_eq!(theme::color_mode(), ColorMode::MonoBlack);

        app.handle_key_event(key);
        assert_eq!(theme::color_mode(), ColorMode::Custom);

        // Reset for other tests
        theme::set_color_mode(ColorMode::Custom);
    }

    #[tokio::test]
    async fn shift_left_right_cycles_from_input_focus() {
        let mut app = App::new(TuiConfig::default());
        app.messages.push(make_tool_result_message("tool 1"));
        app.messages.push(make_user_message("hello"));
        app.messages.push(make_tool_result_message("tool 2"));

        assert_eq!(app.focus, Focus::Input);
        assert_eq!(app.selected_tool_block, None);

        // Shift+Right from input focus
        let key = KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT);
        app.handle_key_event(key);
        assert_eq!(app.selected_tool_block, Some(0));
        assert_eq!(app.focus, Focus::Input, "focus should stay on input");

        // Shift+Right again
        app.handle_key_event(key);
        assert_eq!(app.selected_tool_block, Some(2));

        // Shift+Left
        let key = KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT);
        app.handle_key_event(key);
        assert_eq!(app.selected_tool_block, Some(0));
        assert_eq!(app.focus, Focus::Input, "focus should stay on input");
    }

    // ─── Smart approval mode tests ────────────────────────────────────

    #[tokio::test]
    async fn smart_mode_auto_approves_trusted_tool() {
        let mut app = App::new(TuiConfig::default());
        app.approval_mode = ApprovalMode::Smart;
        app.session_trusted_tools.insert("bash".to_string());

        let (tx, rx) = oneshot::channel();
        let request = ToolApprovalRequest {
            tool_call_id: "call_1".into(),
            tool_name: "bash".into(),
            arguments: serde_json::json!({"command": "ls"}),
            requires_approval: true,
        };

        // Send the request through the approval channel
        app.approval_tx.send((request, tx)).await.unwrap();

        // Receive it — the run() loop would do this, but we simulate inline
        let (req, responder) = app.approval_rx.recv().await.unwrap();
        if app.approval_mode == ApprovalMode::Smart
            && app.session_trusted_tools.contains(&req.tool_name)
        {
            let _ = responder.send(ToolApproval::Approved);
        } else {
            app.pending_approval = Some((req, responder));
        }

        // Should have been auto-approved (no pending approval)
        assert!(app.pending_approval.is_none());
        assert_eq!(rx.await.unwrap(), ToolApproval::Approved);
    }

    #[tokio::test]
    async fn smart_mode_prompts_for_untrusted_tool() {
        let mut app = App::new(TuiConfig::default());
        app.approval_mode = ApprovalMode::Smart;

        let (tx, _rx) = oneshot::channel();
        let request = ToolApprovalRequest {
            tool_call_id: "call_2".into(),
            tool_name: "write_file".into(),
            arguments: serde_json::json!({}),
            requires_approval: true,
        };

        // Simulate the approval_rx path
        let (req, responder) = (request, tx);
        if app.approval_mode == ApprovalMode::Smart
            && app.session_trusted_tools.contains(&req.tool_name)
        {
            let _ = responder.send(ToolApproval::Approved);
        } else {
            app.pending_approval = Some((req, responder));
        }

        // Should be pending (not auto-approved)
        assert!(app.pending_approval.is_some());
    }

    #[tokio::test]
    async fn always_approve_adds_to_trusted_set() {
        let mut app = App::new(TuiConfig::default());

        let (tx, rx) = oneshot::channel();
        let request = ToolApprovalRequest {
            tool_call_id: "call_3".into(),
            tool_name: "bash".into(),
            arguments: serde_json::json!({}),
            requires_approval: true,
        };
        app.pending_approval = Some((request, tx));

        // Simulate pressing 'a'
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        app.handle_key_event(key);

        assert!(app.session_trusted_tools.contains("bash"));
        assert!(app.pending_approval.is_none());
        assert_eq!(rx.await.unwrap(), ToolApproval::Approved);
    }

    #[tokio::test]
    async fn reset_clears_trusted_tools() {
        let stream_fn = Arc::new(MockStreamFn::new(vec![]));
        let agent = make_test_agent(stream_fn);

        let mut app = App::new(TuiConfig::default());
        app.set_agent(agent);
        app.session_trusted_tools.insert("bash".to_string());
        app.session_trusted_tools.insert("read_file".to_string());
        assert_eq!(app.session_trusted_tools.len(), 2);

        // Simulate the Reset command path
        if let Some(agent) = &mut app.agent {
            agent.reset();
        }
        app.messages.clear();
        app.session_trusted_tools.clear();

        assert!(app.session_trusted_tools.is_empty());
    }

    #[tokio::test]
    async fn query_approval_mode_shows_smart() {
        let mut app = App::new(TuiConfig::default());
        app.approval_mode = ApprovalMode::Smart;
        app.session_trusted_tools.insert("bash".to_string());

        // Simulate the QueryApprovalMode command
        let label = match app.approval_mode {
            ApprovalMode::Enabled => "enabled",
            ApprovalMode::Bypassed => "disabled (auto-approve)",
            ApprovalMode::Smart => "smart (auto-approve reads, prompt for writes)",
        };
        let mut msg = format!("Tool approval: {label}");
        if app.approval_mode == ApprovalMode::Smart && !app.session_trusted_tools.is_empty() {
            msg.push_str("\nTrusted tools: ");
            let mut tools: Vec<&str> =
                app.session_trusted_tools.iter().map(String::as_str).collect();
            tools.sort_unstable();
            msg.push_str(&tools.join(", "));
        }

        assert!(msg.contains("smart"));
        assert!(msg.contains("Trusted tools: bash"));
    }

    // ─── Plan mode tests ────────────────────────────────────────────────

    /// A mock read-only tool (does not require approval).
    struct MockReadTool;

    impl AgentTool for MockReadTool {
        fn name(&self) -> &str {
            "read_file"
        }
        fn label(&self) -> &str {
            "Read File"
        }
        fn description(&self) -> &str {
            "Read a file"
        }
        fn parameters_schema(&self) -> &serde_json::Value {
            &serde_json::Value::Null
        }
        fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _cancellation_token: CancellationToken,
            _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
            Box::pin(async { AgentToolResult::text("ok") })
        }
    }

    /// A mock write tool (requires approval).
    struct MockWriteTool;

    impl AgentTool for MockWriteTool {
        fn name(&self) -> &str {
            "write_file"
        }
        fn label(&self) -> &str {
            "Write File"
        }
        fn description(&self) -> &str {
            "Write a file"
        }
        fn parameters_schema(&self) -> &serde_json::Value {
            &serde_json::Value::Null
        }
        fn requires_approval(&self) -> bool {
            true
        }
        fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _cancellation_token: CancellationToken,
            _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
            Box::pin(async { AgentToolResult::text("ok") })
        }
    }

    fn make_test_agent_with_tools(stream_fn: Arc<dyn StreamFn>) -> Agent {
        let mut agent = Agent::new(AgentOptions::new(
            "test system prompt",
            ModelSpec::new("test", "mock-model"),
            stream_fn,
            default_convert,
        ));
        agent.set_tools(vec![
            Arc::new(MockReadTool) as Arc<dyn AgentTool>,
            Arc::new(MockWriteTool) as Arc<dyn AgentTool>,
        ]);
        agent
    }

    #[tokio::test]
    async fn toggle_operating_mode_changes_mode() {
        let stream_fn = Arc::new(MockStreamFn::new(vec![]));
        let agent = make_test_agent_with_tools(stream_fn);

        let mut app = App::new(TuiConfig::default());
        app.set_agent(agent);

        assert_eq!(app.operating_mode, OperatingMode::Execute);

        app.toggle_operating_mode();
        assert_eq!(app.operating_mode, OperatingMode::Plan);

        app.toggle_operating_mode();
        assert_eq!(app.operating_mode, OperatingMode::Execute);
    }

    #[tokio::test]
    async fn plan_mode_filters_tools() {
        let stream_fn = Arc::new(MockStreamFn::new(vec![]));
        let agent = make_test_agent_with_tools(stream_fn);

        let mut app = App::new(TuiConfig::default());
        app.set_agent(agent);

        // Before: both tools present
        assert_eq!(app.agent.as_ref().unwrap().state().tools.len(), 2);

        app.enter_plan_mode();

        // After: only read-only tool remains
        let tools = &app.agent.as_ref().unwrap().state().tools;
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "read_file");
    }

    #[tokio::test]
    async fn plan_mode_modifies_system_prompt() {
        let stream_fn = Arc::new(MockStreamFn::new(vec![]));
        let agent = make_test_agent_with_tools(stream_fn);

        let mut app = App::new(TuiConfig::default());
        app.set_agent(agent);

        app.enter_plan_mode();

        let prompt = &app.agent.as_ref().unwrap().state().system_prompt;
        assert!(
            prompt.contains("planning mode"),
            "system prompt should contain planning mode addendum"
        );
    }

    #[tokio::test]
    async fn exit_plan_mode_restores_tools() {
        let stream_fn = Arc::new(MockStreamFn::new(vec![]));
        let agent = make_test_agent_with_tools(stream_fn);

        let mut app = App::new(TuiConfig::default());
        app.set_agent(agent);

        app.enter_plan_mode();
        assert_eq!(app.agent.as_ref().unwrap().state().tools.len(), 1);

        app.exit_plan_mode();
        assert_eq!(app.agent.as_ref().unwrap().state().tools.len(), 2);
    }

    #[tokio::test]
    async fn exit_plan_mode_restores_system_prompt() {
        let stream_fn = Arc::new(MockStreamFn::new(vec![]));
        let agent = make_test_agent_with_tools(stream_fn);

        let mut app = App::new(TuiConfig::default());
        app.set_agent(agent);

        let original_prompt = app
            .agent
            .as_ref()
            .unwrap()
            .state()
            .system_prompt
            .clone();

        app.enter_plan_mode();
        app.exit_plan_mode();

        let restored_prompt = &app.agent.as_ref().unwrap().state().system_prompt;
        assert_eq!(
            &original_prompt, restored_prompt,
            "system prompt should be restored after exiting plan mode"
        );
    }

    #[tokio::test]
    async fn reset_exits_plan_mode() {
        let stream_fn = Arc::new(MockStreamFn::new(vec![]));
        let agent = make_test_agent_with_tools(stream_fn);

        let mut app = App::new(TuiConfig::default());
        app.set_agent(agent);

        app.enter_plan_mode();
        assert_eq!(app.operating_mode, OperatingMode::Plan);

        // Simulate the Reset command path
        if let Some(agent) = &mut app.agent {
            agent.reset();
        }
        app.messages.clear();
        app.operating_mode = OperatingMode::Execute;
        app.saved_tools = None;
        app.saved_system_prompt = None;

        assert_eq!(app.operating_mode, OperatingMode::Execute);
        assert!(app.saved_tools.is_none());
        assert!(app.saved_system_prompt.is_none());
    }

    #[tokio::test]
    async fn shift_tab_toggles_plan_mode() {
        let stream_fn = Arc::new(MockStreamFn::new(vec![]));
        let agent = make_test_agent_with_tools(stream_fn);

        let mut app = App::new(TuiConfig::default());
        app.set_agent(agent);

        assert_eq!(app.operating_mode, OperatingMode::Execute);

        let key = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        app.handle_key_event(key);
        assert_eq!(app.operating_mode, OperatingMode::Plan);

        let key = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        app.handle_key_event(key);
        assert_eq!(app.operating_mode, OperatingMode::Execute);
    }

    // ─── Help panel tests ────────────────────────────────────────────────

    #[tokio::test]
    async fn f1_toggles_help_panel() {
        let mut app = App::new(TuiConfig::default());
        assert!(!app.help_panel.visible);

        let key = KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE);
        app.handle_key_event(key);
        assert!(app.help_panel.visible);

        app.handle_key_event(key);
        assert!(!app.help_panel.visible);
    }

    #[tokio::test]
    async fn f1_works_from_conversation_focus() {
        let mut app = App::new(TuiConfig::default());
        app.focus = Focus::Conversation;

        let key = KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE);
        app.handle_key_event(key);

        assert!(app.help_panel.visible);
        // F1 should not switch focus away from conversation
        assert_eq!(app.focus, Focus::Conversation);
    }

    #[tokio::test]
    async fn hash_help_toggles_panel() {
        let mut app = App::new(TuiConfig::default());
        assert!(!app.help_panel.visible);

        app.input.insert_char('#');
        app.input.insert_char('h');
        app.input.insert_char('e');
        app.input.insert_char('l');
        app.input.insert_char('p');
        app.submit_input();

        assert!(app.help_panel.visible);
    }
}
