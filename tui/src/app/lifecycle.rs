//! App lifecycle and state helper methods.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use tokio::sync::{mpsc, oneshot};

use swink_agent::{Agent, ApprovalMode, ThinkingLevel, ToolApproval, ToolApprovalRequest};

use crate::config::TuiConfig;
use crate::session::JsonlSessionStore;
use crate::theme;
use crate::ui::conversation::ConversationView;
use crate::ui::help_panel::HelpPanel;
use crate::ui::input::InputEditor;
use crate::ui::tool_panel::ToolPanel;

use super::state::{AgentStatus, App, DisplayMessage, Focus, MessageRole, OperatingMode};
use super::{AUTO_COLLAPSE_SECS, MAX_VISIBLE_TURNS};

impl App {
    pub fn new(config: TuiConfig) -> Self {
        let (agent_tx, agent_rx) = mpsc::channel(256);
        let (approval_tx, approval_rx) = mpsc::channel(16);
        let session_store =
            JsonlSessionStore::default_dir().and_then(|dir| JsonlSessionStore::new(dir).ok());
        let session_id = if session_store.is_some() {
            JsonlSessionStore::new_session_id()
        } else {
            "unnamed".to_string()
        };

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
            session_meta: None,
            approval_rx,
            approval_tx,
            pending_approval: None,
            context_budget: 0,
            context_tokens_used: 0,
            selected_tool_block: None,
            open_editor_requested: false,
            session_trusted_tools: HashSet::new(),
            trust_follow_up: None,
            operating_mode: OperatingMode::Execute,
            pending_plan_approval: false,
            plan_session_start: None,
            available_models: Vec::new(),
            model_index: 0,
            pending_model: None,
            saved_tools: None,
            saved_system_prompt: None,
            conversation_area: Rect::new(0, 0, 0, 0),
            conversation_visible_height: 0,
            pending_steered: Vec::new(),
            steered_fade_ticks: 0,
            selection: None,
        }
    }

    /// Replace the session store and session ID assigned by [`App::new`].
    ///
    /// Consuming builder so it chains cleanly after `App::new(config)`. Use this
    /// when embedding the TUI with a custom storage directory or ID prefix (e.g.
    /// `tui_chat_<uuid>`).
    #[must_use]
    pub fn with_session_store(mut self, store: JsonlSessionStore, id: String) -> Self {
        self.session_store = Some(store);
        self.session_id = id;
        self.session_meta = None;
        self
    }

    pub(super) fn push_system_message(&mut self, content: String) {
        self.messages
            .push(DisplayMessage::new(MessageRole::System, content));
    }

    pub(super) fn abort_agent(&mut self) {
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
        if self.tick_count.is_multiple_of(23) {
            self.blink_on = !self.blink_on;
            if self.status == AgentStatus::Running {
                self.dirty = true;
            }
        }
        self.tool_panel.tick();
        if self.tool_panel.is_visible() {
            self.dirty = true;
        }

        // Auto-dismiss trust follow-up after expiration.
        if let Some(ref follow_up) = self.trust_follow_up
            && follow_up.expires_at < Instant::now()
        {
            self.trust_follow_up = None;
            self.dirty = true;
        }

        // Tick down the steered-message fade-out overlay.
        if self.steered_fade_ticks > 0 {
            self.steered_fade_ticks -= 1;
            self.dirty = true;
        }

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

    pub(super) fn session_info(&self) -> String {
        format!(
            "Model: {}\nInput tokens: {}\nOutput tokens: {}\nCost: ${:.4}\nMessages: {}",
            self.model_name,
            self.total_input_tokens,
            self.total_output_tokens,
            self.total_cost,
            self.messages.len(),
        )
    }

    /// Set the agent instance for this app.
    pub fn set_agent(&mut self, agent: Agent) {
        self.model_name.clone_from(&agent.state().model.model_id);
        self.available_models
            .clone_from(&agent.state().available_models);
        self.model_index = 0;
        self.context_budget = 100_000;
        self.agent = Some(agent);
    }

    /// Return the current approval mode.
    ///
    /// Reads through to the underlying [`Agent`]. Before [`set_agent`](Self::set_agent) is
    /// called, returns [`ApprovalMode::default()`] (Smart). Pass the desired mode to
    /// [`swink_agent::AgentOptions::with_approval_mode`] before constructing the agent to
    /// control startup behavior.
    pub fn approval_mode(&self) -> ApprovalMode {
        self.agent
            .as_ref()
            .map(Agent::approval_mode)
            .unwrap_or_default()
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
    pub(super) fn select_prev_tool_block(&mut self) -> bool {
        let tool_indices: Vec<usize> = self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, message)| message.role == MessageRole::ToolResult)
            .map(|(index, _)| index)
            .collect();
        if tool_indices.is_empty() {
            return false;
        }
        match self.selected_tool_block {
            None => {
                self.selected_tool_block = Some(*tool_indices.last().unwrap());
            }
            Some(current) => {
                if let Some(prev) = tool_indices.iter().rev().find(|&&index| index < current) {
                    self.selected_tool_block = Some(*prev);
                }
            }
        }
        true
    }

    /// Select the next tool result block. Returns true if a tool block exists.
    pub(super) fn select_next_tool_block(&mut self) -> bool {
        let tool_indices: Vec<usize> = self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, message)| message.role == MessageRole::ToolResult)
            .map(|(index, _)| index)
            .collect();
        if tool_indices.is_empty() {
            return false;
        }
        match self.selected_tool_block {
            None => {
                self.selected_tool_block = Some(tool_indices[0]);
            }
            Some(current) => {
                if let Some(next) = tool_indices.iter().find(|&&index| index > current) {
                    self.selected_tool_block = Some(*next);
                }
            }
        }
        true
    }

    /// Cycle to the next available model. Updates the status bar immediately;
    /// the model change is applied on the next [`send_to_agent`](Self::send_to_agent) call.
    pub(super) fn cycle_model(&mut self) {
        if self.available_models.len() <= 1 {
            return;
        }
        self.model_index = (self.model_index + 1) % self.available_models.len();
        let next = self.available_models[self.model_index].clone();
        self.model_name.clone_from(&next.model_id);
        self.pending_model = Some(next);
    }

    /// Update the current thinking level for the active and next-turn model.
    pub(super) fn set_thinking_level(&mut self, level: ThinkingLevel) {
        if let Some(agent) = &mut self.agent {
            agent.set_thinking_level(level);
        }
        if let Some(model) = self.available_models.get_mut(self.model_index) {
            model.thinking_level = level;
        }
        if let Some(model) = &mut self.pending_model {
            model.thinking_level = level;
        }
        self.config.show_thinking = level != ThinkingLevel::Off;
    }

    /// Actual visible height of the current conversation viewport.
    pub(super) fn conversation_page_height(&self) -> usize {
        self.conversation_visible_height.max(1)
    }

    pub(super) fn trim_messages_to_recent_turns(&mut self) {
        let user_indices: Vec<usize> = self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, message)| message.role == MessageRole::User)
            .map(|(index, _)| index)
            .collect();

        if user_indices.len() <= MAX_VISIBLE_TURNS {
            self.conversation
                .clamp_scroll_offset(self.conversation_page_height());
            return;
        }

        let trim_start = user_indices[user_indices.len() - MAX_VISIBLE_TURNS];
        self.messages.drain(0..trim_start);

        self.selected_tool_block = self
            .selected_tool_block
            .and_then(|index| index.checked_sub(trim_start))
            .filter(|&index| {
                self.messages
                    .get(index)
                    .is_some_and(|message| message.role == MessageRole::ToolResult)
            });

        self.conversation
            .clamp_scroll_offset(self.conversation_page_height());
    }
}
