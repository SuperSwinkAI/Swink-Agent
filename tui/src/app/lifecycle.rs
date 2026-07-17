//! App lifecycle and state helper methods.

use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};

use swink_agent::{Agent, ApprovalMode, ThinkingLevel, ToolApproval, ToolApprovalRequest};

use crate::config::TuiConfig;
use crate::session::JsonlSessionStore;
use crate::theme;
use crate::ui::conversation::ConversationView;

use super::state::{
    AgentIo, AgentStatus, App, DisplayMessage, EditorState, MessageRole, ModeState,
    SessionPersistence, UsageTotals, ViewState,
};
use super::{AUTO_COLLAPSE_SECS, MAX_VISIBLE_TURNS};

impl App {
    pub fn new(config: TuiConfig) -> Self {
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
            view: ViewState::new(),
            editor: EditorState::new(),
            agent_io: AgentIo::new(),
            mode: ModeState::new(config.default_model.clone()),
            session: SessionPersistence::new(session_store, session_id),
            usage: UsageTotals::default(),
            config,
            extensions: crate::extensions::TuiExtensions::new(),
        }
    }

    /// Install host-supplied extension points, replacing any already set.
    ///
    /// Consuming builder so it chains after `App::new(config)`, alongside
    /// [`with_session_store`](Self::with_session_store). See
    /// [`TuiExtensions`](crate::TuiExtensions) for what can be registered, and
    /// [`launch_with_extensions`](crate::launch_with_extensions) for the
    /// one-call path.
    ///
    /// # Example
    /// ```rust
    /// use swink_agent_tui::{App, CustomCommandOutcome, TuiConfig, TuiExtensions};
    ///
    /// let extensions = TuiExtensions::new().with_command("turns", |app, _args| {
    ///     CustomCommandOutcome::Feedback(format!("{} turn(s)", app.usage.turn_usage.len()))
    /// });
    /// let app = App::new(TuiConfig::default()).with_extensions(extensions);
    /// ```
    #[must_use]
    pub fn with_extensions(mut self, extensions: crate::extensions::TuiExtensions) -> Self {
        self.extensions = extensions;
        self
    }

    /// Route agent I/O through `transport`, replacing the default in-process wiring.
    ///
    /// Once installed, submitted prompts are forwarded through
    /// [`TuiTransport::send`](crate::transport::TuiTransport::send) — the
    /// backend behind the transport decides whether a message starts a new
    /// turn or steers a running one — and the event loop consumes
    /// [`AgentEvent`](swink_agent::AgentEvent)s from
    /// [`TuiTransport::recv`](crate::transport::TuiTransport::recv). Turn I/O
    /// no longer touches the in-process [`Agent`], so install either an agent
    /// (via [`set_agent`](Self::set_agent)) or a transport, not both.
    ///
    /// Without this call the default path is unchanged: `App` drives the
    /// agent passed to [`set_agent`](Self::set_agent) directly and receives
    /// its events over an internal channel wrapped in an
    /// [`InProcessTransport`](crate::transport::InProcessTransport).
    ///
    /// Consuming builder so it chains after `App::new(config)`, alongside
    /// [`with_extensions`](Self::with_extensions) and
    /// [`with_session_store`](Self::with_session_store).
    #[must_use]
    pub fn with_transport(mut self, transport: Box<dyn crate::transport::TuiTransport>) -> Self {
        self.agent_io.transport = transport;
        self.agent_io.external_transport = true;
        self
    }

    /// Replace the session store and session ID assigned by [`App::new`].
    ///
    /// Consuming builder so it chains cleanly after `App::new(config)`. Use this
    /// when embedding the TUI with a custom storage directory or ID prefix (e.g.
    /// `tui_chat_<uuid>`).
    #[must_use]
    pub fn with_session_store(mut self, store: JsonlSessionStore, id: String) -> Self {
        self.session.session_store = Some(store);
        self.session.session_id = id;
        self.session.session_meta = None;
        self
    }

    pub(super) fn push_system_message(&mut self, content: String) {
        self.view
            .messages
            .push(DisplayMessage::new(MessageRole::System, content));
    }

    pub(super) fn abort_agent(&mut self) {
        if let Some(agent) = &mut self.agent_io.agent {
            agent.abort();
        }
        self.agent_io.status = AgentStatus::Aborted;
        if let Some(msg) = self.view.messages.last_mut()
            && msg.is_streaming
        {
            msg.is_streaming = false;
            msg.content.push_str("\n[aborted]");
        }
        self.view.dirty = true;
    }

    pub(super) fn reset_session_state(&mut self) {
        self.restore_plan_mode_state();
        if let Some(agent) = &mut self.agent_io.agent {
            agent.reset();
        }
        self.view.messages.clear();
        self.view.conversation = ConversationView::new();
        self.usage.total_input_tokens = 0;
        self.usage.total_output_tokens = 0;
        self.usage.total_cost = 0.0;
        self.usage.turn_usage.clear();
        self.usage.context_tokens_used = 0;
        self.agent_io.session_trusted_tools.clear();
        self.agent_io.trust_follow_up = None;
        self.mode.model_index = 0;
        self.mode.pending_model = None;
        self.push_system_message("Agent state reset.".to_string());
    }

    /// Tick handler for animations.
    pub fn tick(&mut self) {
        self.view.tick_count += 1;
        if self.view.tick_count.is_multiple_of(23) {
            self.view.blink_on = !self.view.blink_on;
            if self.agent_io.status == AgentStatus::Running {
                self.view.dirty = true;
            }
        }
        self.view.tool_panel.tick();
        if self.view.tool_panel.is_visible() {
            self.view.dirty = true;
        }

        // Auto-dismiss trust follow-up after expiration.
        if let Some(ref follow_up) = self.agent_io.trust_follow_up
            && follow_up.expires_at < Instant::now()
        {
            self.agent_io.trust_follow_up = None;
            self.view.dirty = true;
        }

        // Tick down the steered-message fade-out overlay.
        if self.view.steered_fade_ticks > 0 {
            self.view.steered_fade_ticks -= 1;
            self.view.dirty = true;
        }

        for msg in &mut self.view.messages {
            if msg.role == MessageRole::ToolResult
                && !msg.collapsed
                && !msg.user_expanded
                && let Some(expanded_at) = msg.expanded_at
                && expanded_at.elapsed() > Duration::from_secs(AUTO_COLLAPSE_SECS)
            {
                msg.collapsed = true;
                self.view.dirty = true;
            }
        }
    }

    pub(super) fn session_info(&self) -> String {
        format!(
            "Model: {}\nInput tokens: {}\nOutput tokens: {}\nCost: ${:.4}\nMessages: {}",
            self.mode.model_name,
            self.usage.total_input_tokens,
            self.usage.total_output_tokens,
            self.usage.total_cost,
            self.view.messages.len(),
        )
    }

    /// Set the agent instance for this app.
    pub fn set_agent(&mut self, agent: Agent) {
        self.mode
            .model_name
            .clone_from(&agent.state().model.model_id);
        self.mode
            .available_models
            .clone_from(&agent.state().available_models);
        self.mode.model_index = 0;
        self.usage.context_budget = 100_000;
        self.agent_io.agent = Some(agent);
    }

    /// Return the current approval mode.
    ///
    /// Reads through to the underlying [`Agent`]. Before [`set_agent`](Self::set_agent) is
    /// called, returns [`ApprovalMode::default()`] (Smart). Pass the desired mode to
    /// [`swink_agent::AgentOptions::with_approval_mode`] before constructing the agent to
    /// control startup behavior.
    pub fn approval_mode(&self) -> ApprovalMode {
        self.agent_io
            .agent
            .as_ref()
            .map(Agent::approval_mode)
            .unwrap_or_default()
    }

    /// Get a clone of the approval request sender for use in the agent callback.
    pub fn approval_sender(
        &self,
    ) -> mpsc::Sender<(ToolApprovalRequest, oneshot::Sender<ToolApproval>)> {
        self.agent_io.approval_tx.clone()
    }

    /// Toggle collapse state of the tool result at the given message index.
    pub fn toggle_collapse(&mut self, index: usize) {
        if let Some(msg) = self.view.messages.get_mut(index)
            && msg.role == MessageRole::ToolResult
        {
            msg.collapsed = !msg.collapsed;
            msg.user_expanded = !msg.collapsed;
            if !msg.collapsed {
                msg.expanded_at = Some(Instant::now());
            }
            self.view.dirty = true;
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
        match self.view.selected_tool_block {
            None => {
                self.view.selected_tool_block = Some(*tool_indices.last().unwrap());
            }
            Some(current) => {
                if let Some(prev) = tool_indices.iter().rev().find(|&&index| index < current) {
                    self.view.selected_tool_block = Some(*prev);
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
        match self.view.selected_tool_block {
            None => {
                self.view.selected_tool_block = Some(tool_indices[0]);
            }
            Some(current) => {
                if let Some(next) = tool_indices.iter().find(|&&index| index > current) {
                    self.view.selected_tool_block = Some(*next);
                }
            }
        }
        true
    }

    /// Cycle to the next available model. Updates the status bar immediately;
    /// the model change is applied on the next [`send_to_agent`](Self::send_to_agent) call.
    pub(super) fn cycle_model(&mut self) {
        if self.mode.available_models.len() <= 1 {
            return;
        }
        self.mode.model_index = (self.mode.model_index + 1) % self.mode.available_models.len();
        let next = self.mode.available_models[self.mode.model_index].clone();
        self.mode.model_name.clone_from(&next.model_id);
        self.mode.pending_model = Some(next);
    }

    /// Update the current thinking level for the active and next-turn model.
    pub(super) fn set_thinking_level(&mut self, level: ThinkingLevel) {
        if let Some(agent) = &mut self.agent_io.agent {
            agent.set_thinking_level(level);
        }
        if let Some(model) = self.mode.available_models.get_mut(self.mode.model_index) {
            model.thinking_level = level;
        }
        if let Some(model) = &mut self.mode.pending_model {
            model.thinking_level = level;
        }
        self.config.show_thinking = level != ThinkingLevel::Off;
    }

    /// Actual visible height of the current conversation viewport.
    pub(super) fn conversation_page_height(&self) -> usize {
        self.view.conversation_visible_height.max(1)
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
            self.view
                .conversation
                .clamp_scroll_offset(self.conversation_page_height());
            return;
        }

        let trim_start = user_indices[user_indices.len() - MAX_VISIBLE_TURNS];
        self.view.messages.drain(0..trim_start);

        self.view.selected_tool_block = self
            .selected_tool_block
            .and_then(|index| index.checked_sub(trim_start))
            .filter(|&index| {
                self.view
                    .messages
                    .get(index)
                    .is_some_and(|message| message.role == MessageRole::ToolResult)
            });

        self.view
            .conversation
            .clamp_scroll_offset(self.conversation_page_height());
    }
}
