//! Agent integration and event handling.

use std::time::Instant;

use futures::StreamExt;

use swink_agent::{
    AgentEvent, AgentMessage, AssistantMessageDelta, ContentBlock, LlmMessage, ToolApproval,
    UserMessage,
};

use super::render_helpers::timestamp_now;
use super::state::{AgentStatus, App, DisplayMessage, MessageRole, OperatingMode, TurnUsage};

impl App {
    pub(super) fn send_to_agent(&mut self, text: String) {
        if self.agent.is_none() {
            return;
        }

        // This is the *only* place `@path` mentions are expanded, and it runs
        // once per submitted prompt — never while the user types (issue #1093).
        // `text` stays raw from here on, so the conversation view and the
        // queued-message overlay keep showing `@src/lib.rs` rather than the
        // file content that goes to the model.
        let outbound = self
            .extensions
            .resolve_mentions(&text)
            .unwrap_or_else(|| text.clone());

        let Some(agent) = &mut self.agent else {
            return;
        };

        let user_message = AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text { text: outbound }],
            timestamp: timestamp_now(),
            cache_hint: None,
        }));

        // If a loop is already running, inject the message as a steering event
        // rather than trying to start a second loop (which would error).
        // Store the text so we can promote it into the conversation display at
        // AgentEnd, and so the queued-message overlay can show it in the meantime.
        if self.status == AgentStatus::Running {
            agent.steer(user_message);
            self.pending_steered.push(text);
            return;
        }

        if let Some(pending) = self.pending_model.take() {
            agent.set_model(pending);
        }

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
            Err(error) => {
                self.status = AgentStatus::Error;
                self.messages.push(DisplayMessage::new(
                    MessageRole::Error,
                    format!("Failed to start agent: {error}"),
                ));
            }
        }
    }

    /// Apply one [`AgentEvent`] to the app state.
    ///
    /// This is the only path by which agent activity reaches the TUI: the event
    /// loop pumps the agent's stream through here, and everything the status bar
    /// and `/usage` report — token counters, cost, per-turn breakdown, model
    /// name, tool panel — is derived from it.
    ///
    /// It is public so a host can drive an [`App`] from a stubbed event stream
    /// and assert on the result without a terminal, which is what a TUI smoke
    /// test needs (issue #1084 §3).
    ///
    /// Costs arrive already priced: the agent loop fills in each assistant
    /// message's [`Cost`](swink_agent::Cost) from operator-declared rates or the
    /// model catalog before emitting it. Feeding a stubbed `MessageEnd` with a
    /// zero cost therefore accumulates zero — set the cost on the stub to
    /// exercise the display.
    ///
    /// # Example
    /// ```rust
    /// use swink_agent::{AgentEvent, AssistantMessage, Cost, StopReason, Usage};
    /// use swink_agent_tui::{App, TuiConfig};
    ///
    /// let mut app = App::new(TuiConfig::default());
    /// app.handle_agent_event(AgentEvent::MessageEnd {
    ///     message: AssistantMessage {
    ///         content: vec![],
    ///         provider: "anthropic".to_string(),
    ///         model_id: "claude-sonnet-4-6".to_string(),
    ///         usage: Usage { input: 120, output: 45, ..Usage::default() },
    ///         cost: Cost { total: 0.0042, ..Cost::default() },
    ///         stop_reason: StopReason::Stop,
    ///         error_message: None,
    ///         error_kind: None,
    ///         timestamp: 0,
    ///         cache_hint: None,
    ///     },
    /// });
    ///
    /// assert_eq!(app.total_input_tokens, 120);
    /// assert_eq!(app.total_output_tokens, 45);
    /// assert!((app.total_cost - 0.0042).abs() < 1e-9);
    /// assert_eq!(app.turn_usage.len(), 1);
    /// ```
    #[allow(clippy::too_many_lines)]
    pub fn handle_agent_event(&mut self, event: AgentEvent) {
        if let Some(agent) = &mut self.agent {
            agent.handle_stream_event(&event);
        }
        match event {
            AgentEvent::AgentStart => {
                self.status = AgentStatus::Running;
            }
            AgentEvent::MessageStart => {
                // If any steered messages are waiting, promote them into the
                // conversation NOW — before the assistant response that processes
                // them — so the display order matches the logical turn order.
                if !self.pending_steered.is_empty() {
                    for text in self.pending_steered.drain(..) {
                        self.messages
                            .push(DisplayMessage::new(MessageRole::User, text));
                    }
                    self.steered_fade_ticks = 10;
                }
                let mut msg = DisplayMessage::new(MessageRole::Assistant, String::new());
                msg.is_streaming = true;
                msg.plan_mode = self.operating_mode == OperatingMode::Plan;
                self.messages.push(msg);
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
                let (content, thinking) = DisplayMessage::assistant_content(&message);

                if let Some(msg) = self
                    .messages
                    .last_mut()
                    .filter(|msg| msg.is_streaming && msg.role == MessageRole::Assistant)
                {
                    msg.is_streaming = false;
                    msg.content = content;
                    msg.thinking = thinking;
                } else if !content.is_empty() || thinking.is_some() {
                    let role = if message.stop_reason == swink_agent::StopReason::Error {
                        MessageRole::Error
                    } else {
                        MessageRole::Assistant
                    };
                    let mut msg = DisplayMessage::new(role, content);
                    msg.thinking = thinking;
                    msg.plan_mode = self.operating_mode == OperatingMode::Plan;
                    self.messages.push(msg);
                }
                self.total_input_tokens += message.usage.input;
                self.total_output_tokens += message.usage.output;
                self.total_cost += message.cost.total;
                self.turn_usage.push(TurnUsage::from_message(&message));
                self.context_tokens_used = message.usage.input;
                self.model_name.clone_from(&message.model_id);
            }
            AgentEvent::ToolExecutionStart { id, name, .. } => {
                self.tool_panel.start_tool(id, name);
            }
            AgentEvent::ToolExecutionUpdate { id, name, partial } => {
                self.tool_panel.update_tool(&id, &name, &partial);
            }
            AgentEvent::ToolExecutionEnd { id, is_error, .. } => {
                self.tool_panel.end_tool(&id, is_error);
            }
            AgentEvent::TurnEnd { tool_results, .. } => {
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
                        let mut msg = DisplayMessage::new(role, content);
                        msg.summary = summary;
                        msg.diff_data = crate::ui::diff::DiffData::from_details(&result.details);
                        if role == MessageRole::ToolResult {
                            msg.expanded_at = Some(Instant::now());
                        }
                        self.messages.push(msg);
                    }
                }
                self.trim_messages_to_recent_turns();
            }
            AgentEvent::AgentEnd { .. } => {
                // Safety flush: if the loop ended without a final MessageStart
                // (e.g. cancelled mid-turn), promote any remaining steered messages.
                if !self.pending_steered.is_empty() {
                    for text in self.pending_steered.drain(..) {
                        self.messages
                            .push(DisplayMessage::new(MessageRole::User, text));
                    }
                    self.steered_fade_ticks = 10;
                }
                self.status = AgentStatus::Idle;
                self.retry_attempt = None;
                if let Err(error) = self.auto_save_session() {
                    tracing::warn!(error = %error, "auto-save failed");
                }
                self.trim_messages_to_recent_turns();
            }
            AgentEvent::ToolApprovalRequested {
                id,
                name,
                arguments,
            } => {
                self.tool_panel
                    .set_awaiting_approval(&id, &name, &arguments);
            }
            AgentEvent::ToolApprovalResolved { id, approved, .. } => {
                self.tool_panel.resolve_approval(&id, approved);
            }
            _ => {}
        }
        self.dirty = true;
    }

    pub(super) fn handle_approval_request(
        &mut self,
        request: swink_agent::ToolApprovalRequest,
        responder: tokio::sync::oneshot::Sender<ToolApproval>,
    ) {
        let smart_auto_approved = self.approval_mode() == swink_agent::ApprovalMode::Smart
            && (!request.requires_approval
                || self.session_trusted_tools.contains(&request.tool_name));
        if smart_auto_approved {
            let _ = responder.send(ToolApproval::Approved);
        } else {
            // Clear any active trust follow-up when a new approval arrives.
            self.trust_follow_up = None;
            self.pending_approval = Some((request, responder));
        }
        self.dirty = true;
    }

    /// Toggle between Plan and Execute modes.
    pub(super) fn toggle_operating_mode(&mut self) {
        // Ignore toggle while agent is streaming or plan approval is pending.
        if self.status == AgentStatus::Running || self.pending_plan_approval {
            return;
        }
        match self.operating_mode {
            OperatingMode::Execute => self.enter_plan_mode(),
            OperatingMode::Plan => {
                // Instead of exiting directly, show approval prompt.
                self.pending_plan_approval = true;
            }
        }
        self.dirty = true;
    }

    /// Approve the current plan — exit plan mode and send plan messages as user input.
    pub(super) fn approve_plan(&mut self) {
        self.pending_plan_approval = false;

        let session_start = self.plan_session_start.unwrap_or(0);

        // Collect assistant messages from the active plan-mode session.
        let plan_messages: Vec<String> = self
            .messages
            .iter()
            .skip(session_start)
            .filter(|m| m.plan_mode && m.role == MessageRole::Assistant)
            .map(|m| m.content.clone())
            .collect();

        self.exit_plan_mode();

        // Send concatenated plan as user message if non-empty.
        let plan_text = plan_messages.join("\n\n---\n\n");
        if !plan_text.is_empty() {
            self.messages
                .push(DisplayMessage::new(MessageRole::User, plan_text.clone()));
            self.trim_messages_to_recent_turns();
            self.conversation.auto_scroll = true;
            self.send_to_agent(plan_text);
        }
    }

    /// Reject the plan — stay in plan mode.
    pub(super) const fn reject_plan(&mut self) {
        self.pending_plan_approval = false;
    }

    pub(super) fn enter_plan_mode(&mut self) {
        let Some(agent) = &mut self.agent else {
            return;
        };

        let (saved_tools, saved_prompt) = agent.enter_plan_mode();
        self.saved_tools = Some(saved_tools);
        self.saved_system_prompt = Some(saved_prompt);
        self.plan_session_start = Some(self.messages.len());

        self.operating_mode = OperatingMode::Plan;
        self.push_system_message("Entered plan mode — read-only tools only.".to_string());
    }

    pub(super) fn restore_plan_mode_state(&mut self) {
        if let Some(agent) = &mut self.agent
            && let (Some(tools), Some(prompt)) =
                (self.saved_tools.take(), self.saved_system_prompt.take())
        {
            agent.exit_plan_mode(tools, prompt);
        }

        self.saved_tools = None;
        self.saved_system_prompt = None;
        self.operating_mode = OperatingMode::Execute;
        self.pending_plan_approval = false;
        self.plan_session_start = None;
    }

    pub(super) fn exit_plan_mode(&mut self) {
        self.restore_plan_mode_state();
        self.push_system_message("Exited plan mode — all tools available.".to_string());
    }
}
