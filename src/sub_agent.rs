//! Sub-agent tool wrapper for multi-agent composition.
//!
//! [`SubAgent`] implements [`AgentTool`], allowing an agent to be used as a tool
//! within a parent agent. On each `execute()` call, it constructs a fresh
//! [`Agent`] from a factory closure, runs it, and maps the result.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::agent::{Agent, AgentOptions};
use crate::stream::StreamFn;
use crate::tool::{AgentTool, AgentToolResult};
use crate::types::{AgentResult, ContentBlock, ModelSpec, StopReason};

// ─── Type aliases ───────────────────────────────────────────────────────────

type OptionsFactoryFn = Arc<dyn Fn() -> AgentOptions + Send + Sync>;
type MapResultFn = Arc<dyn Fn(AgentResult) -> AgentToolResult + Send + Sync>;

// ─── SubAgent ───────────────────────────────────────────────────────────────

/// A tool that wraps an agent, enabling multi-agent composition.
///
/// When executed, constructs a fresh [`Agent`] via the `options_factory`,
/// sends the prompt extracted from tool call params, and maps the
/// [`AgentResult`] into an [`AgentToolResult`].
pub struct SubAgent {
    name: String,
    label: String,
    description: String,
    schema: Value,
    requires_approval: bool,
    options_factory: OptionsFactoryFn,
    map_result: MapResultFn,
}

impl SubAgent {
    /// Start building a sub-agent tool with the given identity.
    ///
    /// Defaults to a schema that accepts a `prompt` string parameter.
    /// Use [`with_options`](Self::with_options) to configure the inner agent.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            description: description.into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The prompt to send to the sub-agent"
                    }
                },
                "required": ["prompt"]
            }),
            requires_approval: false,
            options_factory: Arc::new(|| {
                panic!("SubAgent options_factory not configured; call with_options() or simple()")
            }),
            map_result: Arc::new(default_map_result),
        }
    }

    /// Convenience constructor that builds a fully configured sub-agent.
    ///
    /// Creates an `AgentOptions::new_simple()` internally with the provided
    /// system prompt, model, and stream function.
    #[must_use]
    pub fn simple(
        name: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
        system_prompt: impl Into<String>,
        model: ModelSpec,
        stream_fn: Arc<dyn StreamFn>,
    ) -> Self {
        let system_prompt = system_prompt.into();
        Self::new(name, label, description).with_options(move || {
            AgentOptions::new_simple(system_prompt.clone(), model.clone(), Arc::clone(&stream_fn))
        })
    }

    /// Set a custom JSON Schema for the tool parameters.
    #[must_use]
    pub fn with_schema(mut self, schema: Value) -> Self {
        self.schema = schema;
        self
    }

    /// Set whether this tool requires approval before execution.
    #[must_use]
    pub const fn with_requires_approval(mut self, requires: bool) -> Self {
        self.requires_approval = requires;
        self
    }

    /// Set the factory closure that creates agent options for each execution.
    #[must_use]
    pub fn with_options(mut self, f: impl Fn() -> AgentOptions + Send + Sync + 'static) -> Self {
        self.options_factory = Arc::new(f);
        self
    }

    /// Set a custom result mapper from [`AgentResult`] to [`AgentToolResult`].
    #[must_use]
    pub fn with_map_result(
        mut self,
        f: impl Fn(AgentResult) -> AgentToolResult + Send + Sync + 'static,
    ) -> Self {
        self.map_result = Arc::new(f);
        self
    }
}

/// Default result mapper: extracts text from the last assistant message.
fn default_map_result(result: AgentResult) -> AgentToolResult {
    if result.stop_reason == StopReason::Error {
        let error_text = result
            .error
            .unwrap_or_else(|| "sub-agent ended with error".to_owned());
        return AgentToolResult::error(error_text);
    }

    // Extract text from all messages (last assistant message will have the answer)
    let text = result
        .messages
        .iter()
        .rev()
        .find_map(|msg| {
            if let crate::types::AgentMessage::Llm(crate::types::LlmMessage::Assistant(a)) = msg {
                let t = ContentBlock::extract_text(&a.content);
                if t.is_empty() { None } else { Some(t) }
            } else {
                None
            }
        })
        .unwrap_or_else(|| "sub-agent produced no text output".to_owned());

    AgentToolResult::text(text)
}

impl AgentTool for SubAgent {
    fn name(&self) -> &str {
        &self.name
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> &Value {
        &self.schema
    }

    fn requires_approval(&self) -> bool {
        self.requires_approval
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: std::sync::Arc<std::sync::RwLock<crate::SessionState>>,
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
        let options = (self.options_factory)();
        let map_result = Arc::clone(&self.map_result);
        Box::pin(async move {
            let mut agent = Agent::new(options);
            let prompt = params["prompt"].as_str().unwrap_or("").to_owned();
            let result = tokio::select! {
                r = agent.prompt_text(prompt) => r,
                () = cancellation_token.cancelled() => {
                    agent.abort();
                    return AgentToolResult::error("Sub-agent cancelled.");
                }
            };
            match result {
                Ok(r) => map_result(r),
                Err(e) => AgentToolResult::error(format!("Sub-agent error: {e}")),
            }
        })
    }
}

impl std::fmt::Debug for SubAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubAgent")
            .field("name", &self.name)
            .field("label", &self.label)
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}

// ─── Compile-time Send + Sync assertion ─────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SubAgent>();
};
