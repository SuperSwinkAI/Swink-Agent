//! Agent integration and event handling.

use std::time::Instant;

use futures::StreamExt;

use swink_agent::{
    AgentEvent, AgentMessage, AssistantMessageDelta, ContentBlock, LlmMessage, ToolApproval,
    UserMessage,
};

use super::render_helpers::timestamp_now;
use super::state::{
    AgentStatus, App, DisplayMessage, HunkReview, MessageRole, OperatingMode, TurnUsage,
};

impl App {
    pub(super) fn send_to_agent(&mut self, text: String) {
        if !self.agent_io.external_transport && self.agent_io.agent.is_none() {
            return;
        }

        // This is the *only* place `@path` mentions and `/skill` invocations
        // are expanded, and it runs once per submitted prompt — never while
        // the user types (issues #1093, #1092). `text` stays raw from here on,
        // so the conversation view and the queued-message overlay keep showing
        // `@src/lib.rs` / `/deploy` rather than the content that goes to the
        // model.
        let outbound = self
            .extensions
            .resolve_mentions(&text)
            .unwrap_or_else(|| text.clone());

        // ORDER MATTERS: mentions are resolved on the raw text FIRST, and only
        // then is the skill invocation parsed from — and expanded into — the
        // result. The invocation is a leading token, so its span survives
        // mention splices later in the string; and because the skill body is
        // inserted *after* mention scanning, that body is never mention-scanned
        // itself — a SKILL.md containing `@/etc/passwd` must NOT induce a host
        // file read.
        let outbound = self.extensions.resolve_skill(&outbound).unwrap_or(outbound);

        // Host-installed transport: queue the expanded text for the event
        // loop to flush through `TuiTransport::send`. The backend behind the
        // transport decides whether this starts a new turn or steers a
        // running one; mirror the queued-message overlay locally (raw `text`,
        // like the in-process steer path) so the UX matches.
        if self.agent_io.external_transport {
            if self.agent_io.status == AgentStatus::Running {
                self.agent_io.pending_steered.push(text);
            }
            self.agent_io
                .outbound
                .push(crate::transport::UserInput::new(outbound));
            return;
        }

        let Some(agent) = &mut self.agent_io.agent else {
            return;
        };

        let user_message = AgentMessage::Llm(LlmMessage::User(
            UserMessage::new(vec![ContentBlock::Text { text: outbound }])
                .with_timestamp(timestamp_now()),
        ));

        // If a loop is already running, inject the message as a steering event
        // rather than trying to start a second loop (which would error).
        // Store the text so we can promote it into the conversation display at
        // AgentEnd, and so the queued-message overlay can show it in the meantime.
        if self.agent_io.status == AgentStatus::Running {
            agent.steer(user_message);
            self.agent_io.pending_steered.push(text);
            return;
        }

        if let Some(pending) = self.mode.pending_model.take() {
            agent.set_model(pending);
        }

        let input = vec![user_message];
        self.agent_io.status = AgentStatus::Running;
        self.agent_io.retry_attempt = None;

        match agent.prompt_stream(input) {
            Ok(stream) => {
                let tx = self.agent_io.agent_tx.clone();
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
                self.agent_io.status = AgentStatus::Error;
                self.view.messages.push(DisplayMessage::new(
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
    ///     message: AssistantMessage::new(vec![], "anthropic", "claude-sonnet-4-6")
    ///         .with_usage(Usage::default().with_input(120).with_output(45))
    ///         .with_cost(Cost::default().with_total(0.0042))
    ///         .with_stop_reason(StopReason::Stop)
    ///         .with_timestamp(0),
    /// });
    ///
    /// assert_eq!(app.usage.total_input_tokens, 120);
    /// assert_eq!(app.usage.total_output_tokens, 45);
    /// assert!((app.usage.total_cost - 0.0042).abs() < 1e-9);
    /// assert_eq!(app.usage.turn_usage.len(), 1);
    /// ```
    #[allow(clippy::too_many_lines)]
    pub fn handle_agent_event(&mut self, event: AgentEvent) {
        if let Some(agent) = &mut self.agent_io.agent {
            agent.handle_stream_event(&event);
        }
        match event {
            AgentEvent::AgentStart => {
                self.agent_io.status = AgentStatus::Running;
            }
            AgentEvent::MessageStart => {
                // If any steered messages are waiting, promote them into the
                // conversation NOW — before the assistant response that processes
                // them — so the display order matches the logical turn order.
                if !self.agent_io.pending_steered.is_empty() {
                    for text in self.agent_io.pending_steered.drain(..) {
                        self.view
                            .messages
                            .push(DisplayMessage::new(MessageRole::User, text));
                    }
                    self.view.steered_fade_ticks = 10;
                }
                let mut msg = DisplayMessage::new(MessageRole::Assistant, String::new());
                msg.is_streaming = true;
                msg.plan_mode = self.mode.operating_mode == OperatingMode::Plan;
                self.view.messages.push(msg);
            }
            AgentEvent::MessageUpdate { delta } => {
                if let Some(msg) = self.view.messages.last_mut() {
                    match delta {
                        AssistantMessageDelta::Text { delta, .. } => {
                            msg.content.push_str(&delta);
                        }
                        AssistantMessageDelta::Thinking { delta, .. } => {
                            let thinking = msg.thinking.get_or_insert_with(String::new);
                            thinking.push_str(&delta);
                        }
                        // Covers AssistantMessageDelta::ToolCall and, since
                        // that enum is #[non_exhaustive], any future variant.
                        // TRACKING: new delta variants added in core will
                        // silently no-op in the UI until they get an arm
                        // here — audit this match when core grows the enum.
                        _ => {}
                    }
                }
            }
            AgentEvent::MessageEnd { message } => {
                let (content, thinking) = DisplayMessage::assistant_content(&message);

                if let Some(msg) = self
                    .view
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
                    msg.plan_mode = self.mode.operating_mode == OperatingMode::Plan;
                    self.view.messages.push(msg);
                }
                self.usage.total_input_tokens += message.usage.input;
                self.usage.total_output_tokens += message.usage.output;
                self.usage.total_cost += message.cost.total;
                self.usage
                    .turn_usage
                    .push(TurnUsage::from_message(&message));
                self.usage.context_tokens_used = message.usage.input;
                self.mode.model_name.clone_from(&message.model_id);
            }
            AgentEvent::ToolExecutionStart { id, name, .. } => {
                self.view.tool_panel.start_tool(id, name);
            }
            AgentEvent::ToolExecutionUpdate { id, name, partial } => {
                self.view.tool_panel.update_tool(&id, &name, &partial);
            }
            AgentEvent::ToolExecutionEnd { id, is_error, .. } => {
                self.view.tool_panel.end_tool(&id, is_error);
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
                        self.view.messages.push(msg);
                    }
                }
                self.trim_messages_to_recent_turns();
            }
            AgentEvent::AgentEnd { .. } => {
                // Safety flush: if the loop ended without a final MessageStart
                // (e.g. cancelled mid-turn), promote any remaining steered messages.
                if !self.agent_io.pending_steered.is_empty() {
                    for text in self.agent_io.pending_steered.drain(..) {
                        self.view
                            .messages
                            .push(DisplayMessage::new(MessageRole::User, text));
                    }
                    self.view.steered_fade_ticks = 10;
                }
                self.agent_io.status = AgentStatus::Idle;
                self.agent_io.retry_attempt = None;
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
                self.view
                    .tool_panel
                    .set_awaiting_approval(&id, &name, &arguments);
            }
            AgentEvent::ToolApprovalResolved { id, approved, .. } => {
                self.view.tool_panel.resolve_approval(&id, approved);
            }
            _ => {}
        }
        self.view.dirty = true;
    }

    pub(super) fn handle_approval_request(
        &mut self,
        request: swink_agent::ToolApprovalRequest,
        responder: tokio::sync::oneshot::Sender<ToolApproval>,
    ) {
        let smart_auto_approved = self.approval_mode() == swink_agent::ApprovalMode::Smart
            && (!request.requires_approval
                || self
                    .agent_io
                    .session_trusted_tools
                    .contains(&request.tool_name));
        if smart_auto_approved {
            let _ = responder.send(ToolApproval::Approved);
        } else {
            // Clear any active trust follow-up when a new approval arrives.
            self.agent_io.trust_follow_up = None;
            self.agent_io.pending_approval = Some((request, responder));
        }
        self.view.dirty = true;
    }

    /// Whether the pending approval looks reviewable hunk by hunk.
    ///
    /// Used by the renderer to decide whether to advertise `h`, so it must stay
    /// cheap: it inspects the context JSON without cloning content or computing
    /// hunks. `start_hunk_review()` is the authority and may still decline (for
    /// a write whose content is identical to what is on disk, say), in which
    /// case pressing `h` simply does nothing.
    pub fn pending_approval_has_reviewable_diff(&self) -> bool {
        self.agent_io
            .pending_approval
            .as_ref()
            .and_then(|(request, _)| request.context.as_ref())
            .is_some_and(|context| {
                context
                    .get("is_new_file")
                    .and_then(serde_json::Value::as_bool)
                    == Some(false)
                    && context.get("new_content").is_some()
                    && context
                        .get("old_content")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|old| !old.is_empty())
            })
    }

    /// Open a per-hunk review for the pending approval.
    ///
    /// Returns `false` (leaving the plain approval prompt in place) when the
    /// request carries no reviewable diff: no approval pending, no diff context,
    /// a brand-new file, or content that is identical to what is on disk.
    pub(super) fn start_hunk_review(&mut self) -> bool {
        let Some((request, _)) = self.agent_io.pending_approval.as_ref() else {
            return false;
        };
        let Some(context) = request.context.as_ref() else {
            return false;
        };
        let Some(diff) = crate::ui::diff::DiffData::from_details(context) else {
            return false;
        };
        // A new file has no old content to fall back to, so per-hunk rejection
        // is meaningless — the whole-call y/n prompt already covers it.
        if diff.is_new_file {
            return false;
        }
        let hunks = crate::ui::diff::compute_hunks(&diff.old_content, &diff.new_content);
        if hunks.is_empty() {
            return false;
        }

        self.agent_io.hunk_review = Some(HunkReview::new(diff, hunks));
        true
    }

    /// Record a decision for the hunk under the cursor and advance.
    pub(super) fn decide_current_hunk(&mut self, approve: bool) {
        let Some(review) = self.agent_io.hunk_review.as_mut() else {
            return;
        };
        if review.cursor < review.decisions.len() {
            review.decisions[review.cursor] = Some(approve);
            review.cursor += 1;
        }
        if review.cursor >= review.decisions.len() {
            self.finish_hunk_review();
        }
    }

    /// Approve every hunk from the cursor onward and finalize the review.
    pub(super) fn approve_remaining_hunks(&mut self) {
        let Some(review) = self.agent_io.hunk_review.as_mut() else {
            return;
        };
        for decision in review.decisions.iter_mut().skip(review.cursor) {
            *decision = Some(true);
        }
        review.cursor = review.decisions.len();
        self.finish_hunk_review();
    }

    /// Abandon the review and fall back to the whole-call approval prompt.
    ///
    /// The approval request stays pending — the user still has to answer it.
    pub(super) fn cancel_hunk_review(&mut self) {
        self.agent_io.hunk_review = None;
    }

    /// Resolve the pending approval from the collected per-hunk decisions.
    ///
    /// - every hunk approved -> `Approved` (unmodified arguments)
    /// - every hunk rejected -> `Rejected`
    /// - mixed -> `ApprovedWith` carrying content that keeps only the approved
    ///   hunks, plus a follow-up message telling the agent what was reverted
    fn finish_hunk_review(&mut self) {
        let Some(review) = self.agent_io.hunk_review.take() else {
            return;
        };
        let Some((request, responder)) = self.agent_io.pending_approval.take() else {
            return;
        };

        let approvals = review.approvals();
        let rejected = review.rejected_hunks();

        if rejected.is_empty() {
            let _ = responder.send(ToolApproval::Approved);
            self.view.dirty = true;
            return;
        }

        if rejected.len() == approvals.len() {
            let _ = responder.send(ToolApproval::Rejected);
            self.report_rejected_hunks(&review, &rejected, true);
            self.view.dirty = true;
            return;
        }

        let merged = crate::ui::diff::merge_hunks(
            &review.diff.old_content,
            &review.diff.new_content,
            &approvals,
        );

        // Rewrite only the `content` argument, preserving everything else the
        // model passed. If the arguments are not a JSON object we cannot do
        // that safely, so fail closed rather than let the rejected hunks
        // through on the original arguments.
        let mut arguments = request.arguments;
        let Some(object) = arguments.as_object_mut() else {
            let _ = responder.send(ToolApproval::Rejected);
            self.report_rejected_hunks(&review, &rejected, true);
            self.view.dirty = true;
            return;
        };
        object.insert("content".to_string(), serde_json::Value::String(merged));

        let _ = responder.send(ToolApproval::ApprovedWith(arguments));
        self.report_rejected_hunks(&review, &rejected, false);
        self.view.dirty = true;
    }

    /// Show the rejection locally and tell the agent which hunks were reverted.
    ///
    /// The agent is mid-turn while approval is pending, so this goes through
    /// `send_to_agent`, which steers the message in at the next turn boundary.
    fn report_rejected_hunks(&mut self, review: &HunkReview, rejected: &[usize], all: bool) {
        let total = review.decisions.len();
        let list = rejected
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let path = &review.diff.path;

        let notice = if all {
            format!("Rejected all {total} hunk(s) for {path}; the file was left unchanged.")
        } else {
            format!(
                "Rejected hunk(s) {list} of {total} for {path}; \
                 the remaining hunks were applied."
            )
        };
        self.view
            .messages
            .push(DisplayMessage::new(MessageRole::System, notice));
        self.view.conversation.auto_scroll = true;

        let report = if all {
            format!(
                "I rejected all {total} proposed hunk(s) in your write to {path}, \
                 so the file is unchanged on disk. Do not re-apply them without \
                 addressing my concerns."
            )
        } else {
            format!(
                "I reviewed your write to {path} hunk by hunk and rejected hunk(s) \
                 {list} of {total}. Those changes were reverted to the original \
                 content; the other hunks were applied. The file on disk now \
                 differs from what you proposed — re-read it before editing again."
            )
        };
        self.send_to_agent(report);
    }

    /// Toggle between Plan and Execute modes.
    pub(super) fn toggle_operating_mode(&mut self) {
        // Ignore toggle while agent is streaming or plan approval is pending.
        if self.agent_io.status == AgentStatus::Running || self.mode.pending_plan_approval {
            return;
        }
        match self.mode.operating_mode {
            OperatingMode::Execute => self.enter_plan_mode(),
            OperatingMode::Plan => {
                // Instead of exiting directly, show approval prompt.
                self.mode.pending_plan_approval = true;
            }
        }
        self.view.dirty = true;
    }

    /// Approve the current plan — exit plan mode and send plan messages as user input.
    pub(super) fn approve_plan(&mut self) {
        self.mode.pending_plan_approval = false;

        let session_start = self.mode.plan_session_start.unwrap_or(0);

        // Collect assistant messages from the active plan-mode session.
        let plan_messages: Vec<String> = self
            .view
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
            self.view
                .messages
                .push(DisplayMessage::new(MessageRole::User, plan_text.clone()));
            self.trim_messages_to_recent_turns();
            self.view.conversation.auto_scroll = true;
            self.send_to_agent(plan_text);
        }
    }

    /// Reject the plan — stay in plan mode.
    pub(super) const fn reject_plan(&mut self) {
        self.mode.pending_plan_approval = false;
    }

    pub(super) fn enter_plan_mode(&mut self) {
        let Some(agent) = &mut self.agent_io.agent else {
            return;
        };

        let (saved_tools, saved_prompt) = agent.enter_plan_mode();
        self.mode.saved_tools = Some(saved_tools);
        self.mode.saved_system_prompt = Some(saved_prompt);
        self.mode.plan_session_start = Some(self.view.messages.len());

        self.mode.operating_mode = OperatingMode::Plan;
        self.push_system_message("Entered plan mode — read-only tools only.".to_string());
    }

    pub(super) fn restore_plan_mode_state(&mut self) {
        if let Some(agent) = &mut self.agent_io.agent
            && let (Some(tools), Some(prompt)) = (
                self.mode.saved_tools.take(),
                self.mode.saved_system_prompt.take(),
            )
        {
            agent.exit_plan_mode(tools, prompt);
        }

        self.mode.saved_tools = None;
        self.mode.saved_system_prompt = None;
        self.mode.operating_mode = OperatingMode::Execute;
        self.mode.pending_plan_approval = false;
        self.mode.plan_session_start = None;
    }

    pub(super) fn exit_plan_mode(&mut self) {
        self.restore_plan_mode_state();
        self.push_system_message("Exited plan mode — all tools available.".to_string());
    }
}
