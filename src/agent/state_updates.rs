use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use futures::{Stream, StreamExt};

use crate::error::AgentError;
use crate::loop_::AgentEvent;
use crate::types::{AgentMessage, AgentResult, LlmMessage, StopReason, Usage};

use super::Agent;

impl Agent {
    /// Collect a stream to completion, updating agent state along the way.
    pub(super) async fn collect_stream(
        &mut self,
        mut stream: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>,
    ) -> Result<AgentResult, AgentError> {
        let mut all_messages: Vec<AgentMessage> = Vec::new();
        let mut state_messages = self.in_flight_llm_messages.take().unwrap_or_default();
        let mut checkpoint_messages = self
            .in_flight_messages
            .take()
            .unwrap_or_else(|| clone_messages(&state_messages));
        let mut received_full_context = false;
        let mut stop_reason = StopReason::Stop;
        let mut usage = Usage::default();
        let mut cost = crate::types::Cost::default();
        let mut error: Option<String> = None;
        let mut transfer_signal: Option<crate::transfer::TransferSignal> = None;

        while let Some(event) = stream.next().await {
            self.dispatch_event(&event);
            self.update_state_from_event(&event);

            match event {
                AgentEvent::TransferInitiated { signal } => {
                    transfer_signal = Some(signal);
                    stop_reason = StopReason::Transfer;
                }
                AgentEvent::TurnEnd {
                    assistant_message,
                    tool_results,
                    ..
                } => {
                    // Preserve Transfer stop reason set by TransferInitiated event
                    if transfer_signal.is_none() {
                        stop_reason = assistant_message.stop_reason;
                    }
                    usage += assistant_message.usage.clone();
                    cost += assistant_message.cost.clone();
                    if let Some(ref err) = assistant_message.error_message {
                        error = Some(err.clone());
                    }
                    let assistant_llm = LlmMessage::Assistant(assistant_message);
                    state_messages.push(AgentMessage::Llm(assistant_llm.clone()));
                    checkpoint_messages.push(AgentMessage::Llm(assistant_llm.clone()));
                    all_messages.push(AgentMessage::Llm(assistant_llm));
                    for tr in tool_results {
                        state_messages.push(AgentMessage::Llm(LlmMessage::ToolResult(tr.clone())));
                        checkpoint_messages
                            .push(AgentMessage::Llm(LlmMessage::ToolResult(tr.clone())));
                        all_messages.push(AgentMessage::Llm(LlmMessage::ToolResult(tr)));
                    }
                }
                AgentEvent::AgentEnd { messages } => match Arc::try_unwrap(messages) {
                    Ok(returned) => {
                        self.state.messages = returned;
                        received_full_context = true;
                    }
                    Err(messages) => {
                        self.state.messages = clone_messages(messages.as_ref());
                        received_full_context = true;
                    }
                },
                _ => {}
            }
        }

        if !received_full_context {
            self.state.messages = checkpoint_messages;
        }
        self.state.is_running = false;
        self.loop_active.store(false, Ordering::Release);
        self.state.error.clone_from(&error);
        self.idle_notify.notify_waiters();

        Ok(AgentResult {
            messages: all_messages,
            stop_reason,
            usage,
            cost,
            error,
            transfer_signal,
        })
    }

    /// Processes a streaming event, updating [`Agent::state`] and notifying subscribers.
    pub fn handle_stream_event(&mut self, event: &AgentEvent) {
        self.dispatch_event(event);
        self.update_state_from_event(event);

        match event {
            AgentEvent::TurnEnd {
                assistant_message,
                tool_results,
                ..
            } => {
                let msgs = self.in_flight_llm_messages.get_or_insert_with(Vec::new);
                msgs.push(AgentMessage::Llm(LlmMessage::Assistant(
                    assistant_message.clone(),
                )));
                let checkpoint_msgs = self.in_flight_messages.get_or_insert_with(Vec::new);
                checkpoint_msgs.push(AgentMessage::Llm(LlmMessage::Assistant(
                    assistant_message.clone(),
                )));
                for tr in tool_results {
                    msgs.push(AgentMessage::Llm(LlmMessage::ToolResult(tr.clone())));
                    checkpoint_msgs.push(AgentMessage::Llm(LlmMessage::ToolResult(tr.clone())));
                }
                // Capture terminal error so it survives through AgentEnd.
                if let Some(ref err) = assistant_message.error_message {
                    self.state.error = Some(err.clone());
                }
            }
            AgentEvent::AgentEnd { messages } => {
                self.state.messages = clone_messages(messages.as_ref());
                self.in_flight_llm_messages = None;
                self.in_flight_messages = None;
                // Preserve terminal error — do not clear self.state.error.
                self.idle_notify.notify_waiters();
            }
            _ => {}
        }
    }

    fn update_state_from_event(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::MessageStart => {
                self.state.stream_message = None;
            }
            AgentEvent::MessageEnd { message } => {
                self.state.stream_message =
                    Some(AgentMessage::Llm(LlmMessage::Assistant(message.clone())));
            }
            AgentEvent::ToolExecutionStart { id, .. } => {
                self.state.pending_tool_calls.insert(id.clone());
            }
            AgentEvent::TurnEnd { .. } => {
                self.state.pending_tool_calls.clear();
                self.state.stream_message = None;
            }
            AgentEvent::AgentEnd { .. } => {
                self.state.is_running = false;
                self.loop_active.store(false, Ordering::Release);
                self.state.pending_tool_calls.clear();
                self.state.stream_message = None;
            }
            _ => {}
        }
    }
}

fn clone_messages(messages: &[AgentMessage]) -> Vec<AgentMessage> {
    messages
        .iter()
        .filter_map(|message| match message {
            AgentMessage::Llm(llm) => Some(AgentMessage::Llm(llm.clone())),
            AgentMessage::Custom(cm) => cm.clone_box().map_or_else(
                || {
                    tracing::warn!(
                        "CustomMessage {:?} does not support clone_box — dropped during state rebuild",
                        cm
                    );
                    None
                },
                |cloned| Some(AgentMessage::Custom(cloned)),
            ),
        })
        .collect()
}
