use std::sync::Arc;

use crate::loop_::AgentEvent;
use crate::stream::StreamFn;
use crate::tool::{AgentTool, ApprovalMode};
use crate::types::{AgentMessage, ModelSpec, ThinkingLevel};

use super::{Agent, DEFAULT_PLAN_MODE_ADDENDUM};

impl Agent {
    /// Set the system prompt.
    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) {
        self.state.system_prompt = prompt.into();
    }

    /// Set the model specification, swapping the stream function if a matching
    /// model was registered via [`with_available_models`](crate::AgentOptions::with_available_models).
    ///
    /// If the model actually changes, a [`AgentEvent::ModelCycled`] event is
    /// dispatched to all subscribers.
    pub fn set_model(&mut self, model: ModelSpec) {
        let old = self.state.model.clone();
        if let Some((_, stream_fn)) = self.model_stream_fns.iter().find(|(m, _)| *m == model) {
            self.stream_fn = Arc::clone(stream_fn);
        }
        self.state.model = model.clone();
        if old != model {
            let event = AgentEvent::ModelCycled {
                old,
                new: model,
                reason: "set_model".to_string(),
            };
            self.dispatch_event(&event);
        }
    }

    /// Set the model specification and stream function explicitly, bypassing
    /// the [`available_models`](crate::AgentState::available_models) lookup.
    pub fn set_model_with_stream(&mut self, model: ModelSpec, stream_fn: Arc<dyn StreamFn>) {
        let old = self.state.model.clone();
        self.state.model = model.clone();
        self.stream_fn = stream_fn;
        if old != model {
            let event = AgentEvent::ModelCycled {
                old,
                new: model,
                reason: "set_model_with_stream".to_string(),
            };
            self.dispatch_event(&event);
        }
    }

    /// Set the thinking level on the current model.
    pub const fn set_thinking_level(&mut self, level: ThinkingLevel) {
        self.state.model.thinking_level = level;
    }

    /// Replace the tool set.
    pub fn set_tools(&mut self, tools: Vec<Arc<dyn AgentTool>>) {
        self.state.tools = tools;
    }

    /// Add a tool, replacing any existing tool with the same name.
    pub fn add_tool(&mut self, tool: Arc<dyn AgentTool>) {
        let name = tool.name();
        self.state.tools.retain(|t| t.name() != name);
        self.state.tools.push(tool);
    }

    /// Remove a tool by name. Returns `true` if a tool was found and removed.
    pub fn remove_tool(&mut self, name: &str) -> bool {
        let before = self.state.tools.len();
        self.state.tools.retain(|t| t.name() != name);
        self.state.tools.len() < before
    }

    /// Return the current approval mode.
    pub const fn approval_mode(&self) -> ApprovalMode {
        self.approval_mode
    }

    /// Set the approval mode at runtime.
    pub const fn set_approval_mode(&mut self, mode: ApprovalMode) {
        self.approval_mode = mode;
    }

    /// Find a tool by name.
    #[must_use]
    pub fn find_tool(&self, name: &str) -> Option<&Arc<dyn AgentTool>> {
        self.state.tools.iter().find(|t| t.name() == name)
    }

    /// Return tools matching a predicate.
    #[must_use]
    pub fn tools_matching(
        &self,
        predicate: impl Fn(&dyn AgentTool) -> bool,
    ) -> Vec<&Arc<dyn AgentTool>> {
        self.state
            .tools
            .iter()
            .filter(|t| predicate(t.as_ref()))
            .collect()
    }

    /// Return tools belonging to the given namespace.
    #[must_use]
    pub fn tools_in_namespace(&self, namespace: &str) -> Vec<&Arc<dyn AgentTool>> {
        self.state
            .tools
            .iter()
            .filter(|t| {
                t.metadata()
                    .and_then(|m| m.namespace)
                    .is_some_and(|ns| ns == namespace)
            })
            .collect()
    }

    /// Replace the entire message history.
    pub fn set_messages(&mut self, messages: Vec<AgentMessage>) {
        self.state.messages = messages;
    }

    /// Append messages to the history.
    pub fn append_messages(&mut self, messages: Vec<AgentMessage>) {
        self.state.messages.extend(messages);
    }

    /// Clear the message history.
    pub fn clear_messages(&mut self) {
        self.state.messages.clear();
    }

    /// Enter plan mode: restrict to read-only tools and append plan instructions.
    pub fn enter_plan_mode(&mut self) -> (Vec<Arc<dyn AgentTool>>, String) {
        let state = &mut self.state;
        let saved_tools = state.tools.clone();
        let saved_prompt = state.system_prompt.clone();

        let read_only: Vec<Arc<dyn AgentTool>> = saved_tools
            .iter()
            .filter(|tool| !tool.requires_approval())
            .cloned()
            .collect();
        state.tools = read_only;

        let addendum = self
            .plan_mode_addendum
            .as_deref()
            .unwrap_or(DEFAULT_PLAN_MODE_ADDENDUM);
        state.system_prompt = format!("{}{addendum}", state.system_prompt);

        (saved_tools, saved_prompt)
    }

    /// Exit plan mode: restore previously saved tools and system prompt.
    pub fn exit_plan_mode(&mut self, saved_tools: Vec<Arc<dyn AgentTool>>, saved_prompt: String) {
        self.state.tools = saved_tools;
        self.state.system_prompt = saved_prompt;
    }
}
