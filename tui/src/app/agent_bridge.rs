//! Agent integration and event handling.

use std::time::Instant;

use futures::StreamExt;

use swink_agent::{
    AgentEvent, AgentMessage, AssistantMessageDelta, ContentBlock, LlmMessage, ToolApproval,
    UserMessage,
};

use super::render_helpers::timestamp_now;
use super::state::{AgentStatus, App, DisplayMessage, MessageRole, OperatingMode};

impl App {
    pub(super) fn send_to_agent(&mut self, text: String) {
        let Some(agent) = &mut self.agent else {
            return;
        };

        if let Some(pending) = self.pending_model.take() {
            agent.set_model(pending);
        }

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
            Err(error) => {
                self.status = AgentStatus::Error;
                self.messages.push(DisplayMessage {
                    role: MessageRole::Error,
                    content: format!("Failed to start agent: {error}"),
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
    pub(super) fn handle_agent_event(&mut self, event: AgentEvent) {
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
                        let is_tool_result = role == MessageRole::ToolResult;
                        let diff_data = crate::ui::diff::DiffData::from_details(&result.details);
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
                self.trim_messages_to_recent_turns();
            }
            AgentEvent::AgentEnd { .. } => {
                self.status = AgentStatus::Idle;
                self.retry_attempt = None;
                self.auto_save_session();
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
        if self.approval_mode == swink_agent::ApprovalMode::Smart
            && self.session_trusted_tools.contains(&request.tool_name)
        {
            let _ = responder.send(ToolApproval::Approved);
        } else {
            self.pending_approval = Some((request, responder));
        }
        self.dirty = true;
    }

    /// Toggle between Plan and Execute modes.
    pub(super) fn toggle_operating_mode(&mut self) {
        match self.operating_mode {
            OperatingMode::Execute => self.enter_plan_mode(),
            OperatingMode::Plan => self.exit_plan_mode(),
        }
        self.dirty = true;
    }

    pub(super) fn enter_plan_mode(&mut self) {
        let Some(agent) = &mut self.agent else {
            return;
        };

        let (saved_tools, saved_prompt) = agent.enter_plan_mode();
        self.saved_tools = Some(saved_tools);
        self.saved_system_prompt = Some(saved_prompt);

        self.operating_mode = OperatingMode::Plan;
        self.push_system_message("Entered plan mode — read-only tools only.".to_string());
    }

    pub(super) fn exit_plan_mode(&mut self) {
        let Some(agent) = &mut self.agent else {
            return;
        };

        if let (Some(tools), Some(prompt)) =
            (self.saved_tools.take(), self.saved_system_prompt.take())
        {
            agent.exit_plan_mode(tools, prompt);
        }

        self.operating_mode = OperatingMode::Execute;
        self.push_system_message("Exited plan mode — all tools available.".to_string());
    }
}
