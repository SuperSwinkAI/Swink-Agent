//! Top-level application state and event loop.

use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::{mpsc, oneshot};

use swink_agent::{
    Agent, AgentEvent, AgentMessage, ApprovalMode, AssistantMessageDelta, ContentBlock, LlmMessage,
    ToolApproval, ToolApprovalRequest, UserMessage,
};

use tracing::{info, warn};

use crate::commands::{self, ApprovalModeArg, ClipboardContent, CommandResult};
use crate::config::TuiConfig;
use crate::credentials;
use crate::session::SessionManager;
use crate::ui;
use crate::ui::conversation::ConversationView;
use crate::ui::input::InputEditor;
use crate::ui::tool_panel::ToolPanel;

type AppResult<T> = Result<T, Box<dyn std::error::Error>>;

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
pub struct DisplayMessage {
    pub role: MessageRole,
    pub content: String,
    pub thinking: Option<String>,
    pub is_streaming: bool,
}

/// Top-level application state.
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
    session_manager: Option<SessionManager>,
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
}

impl App {
    pub fn new(config: TuiConfig) -> Self {
        let (agent_tx, agent_rx) = mpsc::channel(256);
        let (approval_tx, approval_rx) = mpsc::channel(16);
        let session_manager = SessionManager::new().ok();
        let session_id = SessionManager::new_session_id();
        Self {
            should_quit: false,
            status: AgentStatus::Idle,
            input: InputEditor::new(),
            messages: Vec::new(),
            conversation: ConversationView::new(),
            tool_panel: ToolPanel::new(),
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
            session_manager,
            session_id,
            approval_rx,
            approval_tx,
            pending_approval: None,
            approval_mode: ApprovalMode::default(),
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
                    self.pending_approval = Some((request, responder));
                    self.dirty = true;
                }
                // Tick for animations
                _ = tick_interval.tick() => {
                    self.tick();
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

        // Handle approval Y/N when a tool is pending approval
        if self.pending_approval.is_some() {
            match key.code {
                KeyCode::Char('y' | 'Y') | KeyCode::Enter => {
                    if let Some((_req, responder)) = self.pending_approval.take() {
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
                };
                self.approval_mode = harness_mode;
                if let Some(agent) = &mut self.agent {
                    agent.set_approval_mode(harness_mode);
                }
                let label = match mode {
                    ApprovalModeArg::On => "enabled",
                    ApprovalModeArg::Off => "disabled (auto-approve)",
                };
                self.push_system_message(format!("Tool approval: {label}"));
                return;
            }
            CommandResult::QueryApprovalMode => {
                let label = match self.approval_mode {
                    ApprovalMode::Enabled => "enabled",
                    ApprovalMode::Bypassed => "disabled (auto-approve)",
                };
                self.push_system_message(format!("Tool approval: {label}"));
                return;
            }
        }

        // Add user message to display
        self.messages.push(DisplayMessage {
            role: MessageRole::User,
            content: text.clone(),
            thinking: None,
            is_streaming: false,
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
                        self.messages.push(DisplayMessage {
                            role: if result.is_error {
                                MessageRole::Error
                            } else {
                                MessageRole::ToolResult
                            },
                            content,
                            thinking: None,
                            is_streaming: false,
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
        self.agent = Some(agent);
    }

    /// Get a clone of the approval request sender for use in the agent callback.
    pub fn approval_sender(
        &self,
    ) -> mpsc::Sender<(ToolApprovalRequest, oneshot::Sender<ToolApproval>)> {
        self.approval_tx.clone()
    }

    /// Approximate visible height of conversation area.
    const fn last_visible_height() -> usize {
        20
    }

    // ─── Session persistence ────────────────────────────────────────────

    fn auto_save_session(&self) {
        let Some(ref mgr) = self.session_manager else {
            return;
        };
        let Some(ref agent) = self.agent else {
            return;
        };
        let state = agent.state();
        let _ = mgr.save_session(
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
        let Some(ref mgr) = self.session_manager else {
            warn!("session persistence unavailable");
            self.push_system_message("Session persistence unavailable.".to_string());
            return;
        };
        info!(session_id = %id, "loading session");
        match mgr.load_session(id) {
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
                                });
                            }
                            LlmMessage::Assistant(a) => {
                                self.messages.push(DisplayMessage {
                                    role: MessageRole::Assistant,
                                    content: ContentBlock::extract_text(&a.content),
                                    thinking: None,
                                    is_streaming: false,
                                });
                            }
                            LlmMessage::ToolResult(t) => {
                                let content = ContentBlock::extract_text(&t.content);
                                if !content.is_empty() {
                                    self.messages.push(DisplayMessage {
                                        role: MessageRole::ToolResult,
                                        content,
                                        thinking: None,
                                        is_streaming: false,
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
        let Some(ref mgr) = self.session_manager else {
            self.push_system_message("Session persistence unavailable.".to_string());
            return;
        };
        match mgr.list_sessions() {
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

    use swink_agent::{
        Agent, AgentEvent, AgentMessage, AgentOptions, AssistantMessageEvent, Cost, LlmMessage,
        ModelSpec, StopReason, StreamFn, StreamOptions, Usage,
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
}
