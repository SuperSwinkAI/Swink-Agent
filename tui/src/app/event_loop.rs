//! Event loop and input handling.

use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use futures::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};

use swink_agent::{ApprovalMode, ToolApproval};

use super::state::{Selection, TrustFollowUp};
use crate::commands::{self, ApprovalModeArg, ClipboardContent, CommandResult};
use crate::theme;
use crate::ui;

use super::render_helpers::extract_last_code_block;
use super::state::{AgentStatus, App, DisplayMessage, Focus, MessageRole, OperatingMode};
use super::{AppResult, MOUSE_SCROLL_STEP};

impl App {
    /// Main async event loop using `tokio::select!`.
    pub async fn run(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> AppResult<()> {
        let tick_rate = std::time::Duration::from_millis(self.config.tick_rate_ms);
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
                maybe_event = event_stream.next() => {
                    if let Some(Ok(event)) = maybe_event {
                        self.handle_terminal_event(&event);
                    }
                }
                Some(event) = self.agent_rx.recv() => {
                    self.handle_agent_event(event);
                    // Drain any additional pending agent events before the next
                    // draw so rapid token bursts are batched into a single frame.
                    while let Ok(event) = self.agent_rx.try_recv() {
                        self.handle_agent_event(event);
                    }
                }
                Some((request, responder)) = self.approval_rx.recv() => {
                    self.handle_approval_request(request, responder);
                }
                _ = tick_interval.tick() => {
                    self.tick();
                }
            }

            if self.open_editor_requested {
                self.open_editor_requested = false;
                let editor = crate::editor::resolve_editor(self.config.editor_command.as_deref());

                let _ = crate::restore_terminal();
                let result = crate::editor::open_editor(&editor);

                let _ = crossterm::terminal::enable_raw_mode();
                let _ = crossterm::execute!(
                    std::io::stdout(),
                    crossterm::terminal::EnterAlternateScreen,
                    crossterm::event::EnableMouseCapture
                );
                terminal.clear()?;
                self.dirty = true;
                event_stream = crossterm::event::EventStream::new();

                match result {
                    Ok(Some(content)) => {
                        self.messages
                            .push(DisplayMessage::new(MessageRole::User, content.clone()));
                        self.trim_messages_to_recent_turns();
                        self.conversation.auto_scroll = true;
                        self.send_to_agent(content);
                    }
                    Ok(None) => {
                        self.push_system_message(
                            "Editor closed with empty content — cancelled.".to_string(),
                        );
                    }
                    Err(error) => {
                        self.push_system_message(format!("Editor error: {error}"));
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn handle_terminal_event(&mut self, event: &Event) {
        match event {
            Event::Key(key) => self.handle_key_event(*key),
            Event::Mouse(mouse) => self.handle_mouse_event(*mouse),
            Event::Resize(_, _) => {
                self.dirty = true;
            }
            _ => {}
        }
    }

    pub(super) fn handle_mouse_event(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                if !self.mouse_in_conversation(mouse.column, mouse.row) {
                    return;
                }
                self.selection = None;
                self.conversation.scroll_up(MOUSE_SCROLL_STEP);
                self.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                if !self.mouse_in_conversation(mouse.column, mouse.row) {
                    return;
                }
                self.selection = None;
                self.conversation
                    .scroll_down(MOUSE_SCROLL_STEP, self.conversation_page_height());
                self.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if !self.mouse_in_conversation(mouse.column, mouse.row) {
                    self.selection = None;
                    self.dirty = true;
                    return;
                }
                if let Some((row, col)) = self.inner_conversation_coords(mouse.column, mouse.row) {
                    self.selection = Some(Selection::new(row, col));
                    self.dirty = true;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.selection.is_none() {
                    return;
                }
                let (row, col) = self.clamped_conversation_coords(mouse.column, mouse.row);
                if let Some(sel) = self.selection.as_mut() {
                    sel.cursor = (row, col);
                    sel.dragging = true;
                }
                self.dirty = true;
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(sel) = self.selection.as_mut() {
                    sel.dragging = false;
                }
                self.copy_selection_to_clipboard();
            }
            _ => {}
        }
    }

    const fn mouse_in_conversation(&self, column: u16, row: u16) -> bool {
        let area = self.conversation_area;
        let within_x = column >= area.x && column < area.x.saturating_add(area.width);
        let within_y = row >= area.y && row < area.y.saturating_add(area.height);
        within_x && within_y
    }

    /// Translate absolute terminal coordinates into `(row, col)` inside the
    /// conversation's inner area (i.e. excluding the border). Returns `None`
    /// if the position falls outside the inner area.
    fn inner_conversation_coords(&self, column: u16, row: u16) -> Option<(u16, u16)> {
        let area = self.conversation_area;
        let inner_x = area.x.checked_add(1)?;
        let inner_y = area.y.checked_add(1)?;
        let inner_w = area.width.saturating_sub(2);
        let inner_h = area.height.saturating_sub(2);
        if column < inner_x || row < inner_y {
            return None;
        }
        let col = column - inner_x;
        let r = row - inner_y;
        if col >= inner_w || r >= inner_h {
            return None;
        }
        Some((r, col))
    }

    /// Like [`Self::inner_conversation_coords`] but clamps to the viewport
    /// edges instead of returning `None` — used during drag so the selection
    /// still extends when the cursor leaves the conversation area.
    fn clamped_conversation_coords(&self, column: u16, row: u16) -> (u16, u16) {
        let area = self.conversation_area;
        let inner_x = area.x.saturating_add(1);
        let inner_y = area.y.saturating_add(1);
        let inner_w = area.width.saturating_sub(2);
        let inner_h = area.height.saturating_sub(2);
        let max_col = inner_w.saturating_sub(1);
        let max_row = inner_h.saturating_sub(1);
        let col = column.saturating_sub(inner_x).min(max_col);
        let r = row.saturating_sub(inner_y).min(max_row);
        (r, col)
    }

    /// Copy the current selection (if any) to the system clipboard.
    pub(super) fn copy_selection_to_clipboard(&mut self) {
        let Some(sel) = self.selection else { return };
        let Some(text) = self.conversation.selection_text(&sel) else {
            return;
        };
        match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text)) {
            Ok(()) => {}
            Err(error) => {
                self.push_system_message(format!("Clipboard error: {error}"));
            }
        }
        self.dirty = true;
    }

    pub(super) fn handle_key_event(&mut self, key: KeyEvent) {
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('q')) => {
                self.should_quit = true;
                self.dirty = true;
                return;
            }
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                if self.selection.is_some() {
                    self.copy_selection_to_clipboard();
                    self.selection = None;
                } else if self.status == AgentStatus::Running {
                    self.abort_agent();
                } else {
                    self.should_quit = true;
                }
                self.dirty = true;
                return;
            }
            (_, KeyCode::Esc) if self.selection.is_some() => {
                self.selection = None;
                self.dirty = true;
                return;
            }
            _ => {}
        }

        // Handle modal prompts (trust follow-up, plan approval, tool approval).
        // Returns true if the key was consumed.
        if self.handle_modal_key(key) {
            return;
        }

        if self.focus == Focus::Conversation {
            let page = self.conversation_page_height();
            match key.code {
                KeyCode::Up => self.conversation.scroll_up(1),
                KeyCode::Down => self.conversation.scroll_down(1, page),
                KeyCode::PageUp => self.conversation.scroll_up(page),
                KeyCode::PageDown => self.conversation.scroll_down(page, page),
                KeyCode::F(1) => self.help_panel.toggle(),
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

    /// Handle modal prompts: trust follow-up, plan approval, tool approval.
    /// Returns `true` if the key was consumed by a modal.
    fn handle_modal_key(&mut self, key: KeyEvent) -> bool {
        // Priority 1: Trust follow-up prompt
        if self.trust_follow_up.is_some() {
            match key.code {
                KeyCode::Char('y' | 'Y') | KeyCode::Enter => {
                    if let Some(follow_up) = self.trust_follow_up.take() {
                        self.session_trusted_tools.insert(follow_up.tool_name);
                    }
                    self.dirty = true;
                    return true;
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                    self.trust_follow_up = None;
                    self.dirty = true;
                    return true;
                }
                _ => {
                    // Clear follow-up on any other key, then re-process
                    self.trust_follow_up = None;
                    self.dirty = true;
                    // Fall through to process the key normally
                }
            }
        }

        // Priority 2: Plan approval prompt
        if self.pending_plan_approval {
            match key.code {
                KeyCode::Char('y' | 'Y') | KeyCode::Enter => {
                    self.approve_plan();
                    self.dirty = true;
                    return true;
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                    self.reject_plan();
                    self.dirty = true;
                    return true;
                }
                _ => {
                    return true;
                }
            }
        }

        // Priority 3: Tool approval prompt
        if self.pending_approval.is_some() {
            match key.code {
                KeyCode::Char('y' | 'Y') | KeyCode::Enter => {
                    if let Some((req, responder)) = self.pending_approval.take() {
                        let _ = responder.send(ToolApproval::Approved);
                        // In Smart mode, offer trust follow-up
                        if self.approval_mode() == ApprovalMode::Smart {
                            self.trust_follow_up = Some(TrustFollowUp {
                                tool_name: req.tool_name,
                                expires_at: Instant::now() + Duration::from_secs(3),
                            });
                        }
                    }
                    self.dirty = true;
                    return true;
                }
                KeyCode::Char('a' | 'A') => {
                    if let Some((req, responder)) = self.pending_approval.take() {
                        self.session_trusted_tools.insert(req.tool_name);
                        let _ = responder.send(ToolApproval::Approved);
                    }
                    self.dirty = true;
                    return true;
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                    if let Some((_req, responder)) = self.pending_approval.take() {
                        let _ = responder.send(ToolApproval::Rejected);
                    }
                    self.dirty = true;
                    return true;
                }
                _ => {
                    return true;
                }
            }
        }

        false
    }

    fn handle_input_key(&mut self, key: KeyEvent) {
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) if self.status == AgentStatus::Running => {
                self.abort_agent();
            }
            (KeyModifiers::SHIFT, KeyCode::BackTab) => {
                self.toggle_operating_mode();
            }
            (_, KeyCode::Tab) => {
                self.focus = match self.focus {
                    Focus::Input => Focus::Conversation,
                    Focus::Conversation => Focus::Input,
                };
            }
            (KeyModifiers::NONE, KeyCode::Enter) => {
                self.submit_input();
            }
            (KeyModifiers::SHIFT, KeyCode::Enter) => {
                self.input.insert_newline();
            }
            (_, KeyCode::Home) | (KeyModifiers::CONTROL, KeyCode::Char('a')) => {
                self.input.move_home();
            }
            (_, KeyCode::End) | (KeyModifiers::CONTROL, KeyCode::Char('e')) => {
                self.input.move_end();
            }
            (KeyModifiers::NONE, KeyCode::Up) => {
                if self.input.cursor_row() == 0 {
                    self.input.history_prev();
                } else {
                    self.input.move_up();
                }
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                if self.input.cursor_row() + 1 >= self.input.line_count() {
                    self.input.history_next();
                } else {
                    self.input.move_down();
                }
            }
            (KeyModifiers::NONE, KeyCode::Left) => self.input.move_left(),
            (KeyModifiers::NONE, KeyCode::Right) => self.input.move_right(),
            (_, KeyCode::PageUp) => {
                let page = self.conversation_page_height();
                self.conversation.scroll_up(page);
            }
            (_, KeyCode::PageDown) => {
                let page = self.conversation_page_height();
                self.conversation.scroll_down(page, page);
            }
            (_, KeyCode::Backspace) => self.input.backspace(),
            (_, KeyCode::Delete) => self.input.delete(),
            (_, KeyCode::F(1)) => {
                self.help_panel.toggle();
            }
            (_, KeyCode::F(2)) => {
                let target = self.selected_tool_block.or_else(|| {
                    self.messages
                        .iter()
                        .enumerate()
                        .rev()
                        .find(|(_, message)| message.role == MessageRole::ToolResult)
                        .map(|(index, _)| index)
                });
                if let Some(index) = target {
                    self.toggle_collapse(index);
                    self.selected_tool_block = Some(index);
                }
            }
            (_, KeyCode::F(3)) => {
                theme::cycle_color_mode();
            }
            (_, KeyCode::F(4)) => {
                self.cycle_model();
            }
            (KeyModifiers::SHIFT, KeyCode::Left) => {
                self.select_prev_tool_block();
            }
            (KeyModifiers::SHIFT, KeyCode::Right) => {
                self.select_next_tool_block();
            }
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(character)) => {
                self.input.insert_char(character);
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn submit_input(&mut self) {
        let Some(text) = self.input.submit() else {
            return;
        };

        match commands::execute_command(&text) {
            CommandResult::NotACommand => {}
            CommandResult::Quit => {
                self.should_quit = true;
                return;
            }
            CommandResult::Clear => {
                self.messages.clear();
                self.conversation = crate::ui::conversation::ConversationView::new();
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
            CommandResult::SetThinking(level) => {
                self.push_system_message(format!("Thinking level set to: {level}"));
                return;
            }
            CommandResult::SetSystemPrompt(prompt) => {
                if let Some(agent) = &mut self.agent {
                    agent.set_system_prompt(prompt);
                }
                self.push_system_message("System prompt updated.".to_string());
                self.trim_messages_to_recent_turns();
                return;
            }
            CommandResult::Reset => {
                if let Some(agent) = &mut self.agent {
                    agent.reset();
                }
                self.messages.clear();
                self.conversation = crate::ui::conversation::ConversationView::new();
                self.total_input_tokens = 0;
                self.total_output_tokens = 0;
                self.total_cost = 0.0;
                self.context_tokens_used = 0;
                self.session_trusted_tools.clear();
                self.trust_follow_up = None;
                self.operating_mode = OperatingMode::Execute;
                self.pending_plan_approval = false;
                self.model_index = 0;
                self.pending_model = None;
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
                let _ = self.load_session(&id);
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
                if let Some(agent) = &mut self.agent {
                    agent.set_approval_mode(harness_mode);
                }
                let label = match mode {
                    ApprovalModeArg::On => "enabled",
                    ApprovalModeArg::Off => "disabled (auto-approve)",
                    ApprovalModeArg::Smart => {
                        "smart (auto-approve trusted tools, prompt for untrusted tools)"
                    }
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
            CommandResult::UntrustTool(name) => {
                self.session_trusted_tools.remove(&name);
                self.push_system_message(format!("Untrusted tool: {name}"));
                return;
            }
            CommandResult::UntrustAll => {
                self.session_trusted_tools.clear();
                self.push_system_message("Cleared all trusted tools".to_string());
                return;
            }
            CommandResult::QueryApprovalMode => {
                let label = match self.approval_mode() {
                    ApprovalMode::Enabled => "enabled",
                    ApprovalMode::Bypassed => "disabled (auto-approve)",
                    ApprovalMode::Smart => {
                        "smart (auto-approve trusted tools, prompt for untrusted tools)"
                    }
                    _ => "unknown",
                };
                let mut msg = format!("Tool approval: {label}");
                if self.approval_mode() == ApprovalMode::Smart
                    && !self.session_trusted_tools.is_empty()
                {
                    msg.push_str("\nTrusted tools: ");
                    let mut tools: Vec<&str> = self
                        .session_trusted_tools
                        .iter()
                        .map(String::as_str)
                        .collect();
                    tools.sort_unstable();
                    msg.push_str(&tools.join(", "));
                }
                self.push_system_message(msg);
                return;
            }
        }

        // Only add to the visible conversation immediately when the agent is idle.
        // Mid-stream submissions are held in `pending_steered` by `send_to_agent`
        // and promoted into `messages` at AgentEnd, after the current response.
        if self.status != AgentStatus::Running {
            self.messages
                .push(DisplayMessage::new(MessageRole::User, text.clone()));
            self.trim_messages_to_recent_turns();
        }
        self.conversation.auto_scroll = true;
        self.send_to_agent(text);
    }

    fn copy_to_clipboard(&mut self, content: ClipboardContent) {
        let text = match content {
            ClipboardContent::Last => self
                .messages
                .iter()
                .rev()
                .find(|message| message.role == MessageRole::Assistant)
                .map(|message| message.content.clone()),
            ClipboardContent::All => {
                let all: String = self
                    .messages
                    .iter()
                    .map(|message| {
                        let role = match message.role {
                            MessageRole::User => "You",
                            MessageRole::Assistant => "Assistant",
                            MessageRole::ToolResult => "Tool",
                            MessageRole::Error => "Error",
                            MessageRole::System => "System",
                        };
                        format!("{role}: {}", message.content)
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                Some(all)
            }
            ClipboardContent::Code => self
                .messages
                .iter()
                .rev()
                .find(|message| message.role == MessageRole::Assistant)
                .and_then(|message| extract_last_code_block(&message.content)),
        };

        let feedback = text.map_or_else(
            || "Nothing to copy.".to_string(),
            |text| match arboard::Clipboard::new()
                .and_then(|mut clipboard| clipboard.set_text(text))
            {
                Ok(()) => "Copied to clipboard.".to_string(),
                Err(error) => format!("Clipboard error: {error}"),
            },
        );

        self.push_system_message(feedback);
    }
}
