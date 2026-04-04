//! Pipeline executor and agent factory traits.

use std::collections::HashMap;
use std::sync::Arc;

use swink_agent::{Agent, AgentMessage, AgentResult, ContentBlock, LlmMessage};
use tokio_util::sync::CancellationToken;

use super::events::PipelineEvent;
use super::output::{PipelineError, PipelineOutput, StepResult};
use super::registry::PipelineRegistry;
use super::types::{Pipeline, PipelineId};

// ─── AgentFactory ───────────────────────────────────────────────────────────

/// Trait for creating agents by name during pipeline execution.
pub trait AgentFactory: Send + Sync {
    /// Create an agent with the given name.
    fn create(&self, name: &str) -> Result<Agent, PipelineError>;
}

// ─── SimpleAgentFactory ─────────────────────────────────────────────────────

/// A basic agent factory backed by a name → builder-fn registry.
pub struct SimpleAgentFactory {
    builders: HashMap<String, Arc<dyn Fn() -> Agent + Send + Sync>>,
}

impl SimpleAgentFactory {
    /// Create an empty factory.
    pub fn new() -> Self {
        Self {
            builders: HashMap::new(),
        }
    }

    /// Register a builder function for the given agent name.
    pub fn register(
        &mut self,
        name: impl Into<String>,
        builder: impl Fn() -> Agent + Send + Sync + 'static,
    ) {
        self.builders.insert(name.into(), Arc::new(builder));
    }
}

impl Default for SimpleAgentFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentFactory for SimpleAgentFactory {
    fn create(&self, name: &str) -> Result<Agent, PipelineError> {
        let builder = self
            .builders
            .get(name)
            .ok_or_else(|| PipelineError::AgentNotFound {
                name: name.to_owned(),
            })?;
        Ok(builder())
    }
}

// ─── PipelineExecutor ───────────────────────────────────────────────────────

/// Orchestrates pipeline execution using an agent factory and registry.
pub struct PipelineExecutor {
    factory: Arc<dyn AgentFactory>,
    registry: Arc<PipelineRegistry>,
    event_handler: Option<Arc<dyn Fn(PipelineEvent) + Send + Sync>>,
}

impl PipelineExecutor {
    /// Create a new executor with the given factory and registry.
    pub fn new(factory: Arc<dyn AgentFactory>, registry: Arc<PipelineRegistry>) -> Self {
        Self {
            factory,
            registry,
            event_handler: None,
        }
    }

    /// Set an event handler that receives pipeline lifecycle events.
    #[must_use]
    pub fn with_event_handler(
        mut self,
        handler: impl Fn(PipelineEvent) + Send + Sync + 'static,
    ) -> Self {
        self.event_handler = Some(Arc::new(handler));
        self
    }

    /// Emit a pipeline event to the handler (if set).
    fn emit(&self, event: PipelineEvent) {
        if let Some(handler) = &self.event_handler {
            handler(event);
        }
    }

    /// Run a pipeline by ID.
    pub async fn run(
        &self,
        pipeline_id: &PipelineId,
        input: String,
        cancellation_token: CancellationToken,
    ) -> Result<PipelineOutput, PipelineError> {
        let pipeline =
            self.registry
                .get(pipeline_id)
                .ok_or_else(|| PipelineError::PipelineNotFound {
                    id: pipeline_id.clone(),
                })?;

        match pipeline {
            Pipeline::Sequential {
                id,
                name,
                steps,
                pass_context,
            } => {
                self.run_sequential(id, name, steps, pass_context, input, cancellation_token)
                    .await
            }
            Pipeline::Parallel {
                id,
                name,
                branches,
                merge_strategy,
            } => {
                super::parallel::run_parallel(
                    &self.factory,
                    &self.event_handler,
                    id,
                    name,
                    branches,
                    merge_strategy,
                    input,
                    cancellation_token,
                )
                .await
            }
            Pipeline::Loop {
                id,
                name,
                body,
                exit_condition,
                max_iterations,
            } => {
                super::loop_exec::run_loop(
                    &self.factory,
                    &self.event_handler,
                    id,
                    name,
                    body,
                    exit_condition,
                    max_iterations,
                    input,
                    cancellation_token,
                )
                .await
            }
        }
    }

    async fn run_sequential(
        &self,
        id: PipelineId,
        name: String,
        steps: Vec<String>,
        pass_context: bool,
        input: String,
        cancellation_token: CancellationToken,
    ) -> Result<PipelineOutput, PipelineError> {
        let start = std::time::Instant::now();
        let mut step_results = Vec::new();
        let mut current_input = input;
        let mut total_usage = swink_agent::Usage::default();
        // Accumulated message history for pass_context mode.
        let mut context_messages: Vec<LlmMessage> = Vec::new();

        self.emit(PipelineEvent::Started {
            pipeline_id: id.clone(),
            pipeline_name: name.clone(),
        });

        for (index, agent_name) in steps.iter().enumerate() {
            if cancellation_token.is_cancelled() {
                return Err(PipelineError::Cancelled);
            }

            self.emit(PipelineEvent::StepStarted {
                pipeline_id: id.clone(),
                step_index: index,
                agent_name: agent_name.clone(),
            });

            let step_start = std::time::Instant::now();
            let mut agent = self.factory.create(agent_name)?;

            // Build input messages: either accumulated context or just the current input.
            let messages = if pass_context && !context_messages.is_empty() {
                let mut msgs: Vec<AgentMessage> = context_messages
                    .iter()
                    .map(|llm| AgentMessage::Llm(llm.clone()))
                    .collect();
                msgs.push(user_msg(&current_input));
                msgs
            } else {
                vec![user_msg(&current_input)]
            };

            let result =
                agent
                    .prompt_async(messages)
                    .await
                    .map_err(|e| PipelineError::StepFailed {
                        step_index: index,
                        agent_name: agent_name.clone(),
                        source: Box::new(e),
                    })?;

            let response = extract_text_response(&result);
            let step_duration = step_start.elapsed();

            total_usage += result.usage.clone();

            self.emit(PipelineEvent::StepCompleted {
                pipeline_id: id.clone(),
                step_index: index,
                agent_name: agent_name.clone(),
                duration: step_duration,
                usage: result.usage.clone(),
            });

            step_results.push(StepResult {
                agent_name: agent_name.clone(),
                response: response.clone(),
                duration: step_duration,
                usage: result.usage.clone(),
            });

            // In pass_context mode, accumulate the user message and assistant response.
            if pass_context {
                // Push the user message as an LlmMessage
                context_messages.push(LlmMessage::User(swink_agent::UserMessage {
                    content: vec![ContentBlock::Text {
                        text: current_input.clone(),
                    }],
                    timestamp: 0,
                    cache_hint: None,
                }));
                // Add the assistant messages from the result.
                for msg in &result.messages {
                    if let AgentMessage::Llm(llm @ LlmMessage::Assistant(_)) = msg {
                        context_messages.push(llm.clone());
                    }
                }
            }

            current_input = response;
        }

        let total_duration = start.elapsed();
        let final_response = step_results
            .last()
            .map(|s| s.response.clone())
            .unwrap_or_default();

        self.emit(PipelineEvent::Completed {
            pipeline_id: id.clone(),
            total_duration,
            total_usage: total_usage.clone(),
        });

        Ok(PipelineOutput {
            pipeline_id: id,
            final_response,
            steps: step_results,
            total_duration,
            total_usage,
        })
    }
}

/// Build a user message from text (local helper to avoid testkit dependency).
fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(swink_agent::UserMessage {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        timestamp: 0,
        cache_hint: None,
    }))
}

/// Extract concatenated text content from an agent result's last assistant message.
fn extract_text_response(result: &AgentResult) -> String {
    result
        .messages
        .iter()
        .rev()
        .find_map(|m| match m {
            AgentMessage::Llm(LlmMessage::Assistant(msg)) => Some(msg),
            _ => None,
        })
        .map(|msg| {
            msg.content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

#[cfg(all(test, feature = "testkit"))]
mod tests {
    use super::*;
    use std::sync::Arc;
    use swink_agent::AgentOptions;
    use swink_agent::testing::{MockStreamFn, default_convert, default_model, text_only_events};

    fn make_agent() -> Agent {
        let options = AgentOptions::new(
            "test",
            default_model(),
            Arc::new(MockStreamFn::new(vec![])),
            default_convert,
        );
        Agent::new(options)
    }

    fn make_text_agent(text: &str) -> Agent {
        let events = text_only_events(text);
        let options = AgentOptions::new(
            "test",
            default_model(),
            Arc::new(MockStreamFn::new(vec![events])),
            default_convert,
        );
        Agent::new(options)
    }

    // T017: SimpleAgentFactory tests

    #[test]
    fn factory_create_registered_agent_succeeds() {
        let mut factory = SimpleAgentFactory::new();
        factory.register("test-agent", make_agent);

        let result = factory.create("test-agent");
        assert!(result.is_ok());
    }

    #[test]
    fn factory_create_unknown_returns_agent_not_found() {
        let factory = SimpleAgentFactory::new();

        let result = factory.create("nonexistent");
        assert!(matches!(
            result,
            Err(PipelineError::AgentNotFound { name }) if name == "nonexistent"
        ));
    }

    // T020-T024: Sequential pipeline tests

    fn build_executor(factory: SimpleAgentFactory, registry: PipelineRegistry) -> PipelineExecutor {
        PipelineExecutor::new(Arc::new(factory), Arc::new(registry))
    }

    #[tokio::test]
    async fn sequential_two_step_pipeline() {
        let mut factory = SimpleAgentFactory::new();
        factory.register("agent-a", || make_text_agent("hello"));
        factory.register("agent-b", || make_text_agent("world"));

        let registry = PipelineRegistry::new();
        let pipeline = Pipeline::sequential("two-step", vec!["agent-a".into(), "agent-b".into()]);
        let id = pipeline.id().clone();
        registry.register(pipeline);

        let executor = build_executor(factory, registry);
        let token = CancellationToken::new();

        let output = executor.run(&id, "input".into(), token).await.unwrap();
        assert_eq!(output.final_response, "world");
        assert_eq!(output.steps.len(), 2);
        assert_eq!(output.steps[0].agent_name, "agent-a");
        assert_eq!(output.steps[0].response, "hello");
        assert_eq!(output.steps[1].agent_name, "agent-b");
        assert_eq!(output.steps[1].response, "world");
    }

    #[tokio::test]
    async fn sequential_missing_step_agent_halts_with_error() {
        // agent-b is not registered in the factory, causing AgentNotFound.
        let mut factory = SimpleAgentFactory::new();
        factory.register("agent-a", || make_text_agent("step-one"));
        // agent-b intentionally not registered
        factory.register("agent-c", || make_text_agent("step-three"));

        let registry = PipelineRegistry::new();
        let pipeline = Pipeline::sequential(
            "three-step",
            vec!["agent-a".into(), "agent-b".into(), "agent-c".into()],
        );
        let id = pipeline.id().clone();
        registry.register(pipeline);

        let executor = build_executor(factory, registry);
        let token = CancellationToken::new();

        let result = executor.run(&id, "input".into(), token).await;
        assert!(result.is_err(), "expected error when step agent not found");
        assert!(
            matches!(result.unwrap_err(), PipelineError::AgentNotFound { name } if name == "agent-b"),
            "expected AgentNotFound for agent-b"
        );
    }

    #[tokio::test]
    async fn sequential_missing_agent_returns_agent_not_found() {
        let factory = SimpleAgentFactory::new(); // no agents registered

        let registry = PipelineRegistry::new();
        let pipeline = Pipeline::sequential("missing", vec!["ghost".into()]);
        let id = pipeline.id().clone();
        registry.register(pipeline);

        let executor = build_executor(factory, registry);
        let token = CancellationToken::new();

        let result = executor.run(&id, "input".into(), token).await;
        assert!(matches!(
            result,
            Err(PipelineError::AgentNotFound { name }) if name == "ghost"
        ));
    }

    #[tokio::test]
    async fn sequential_zero_steps_returns_empty() {
        let factory = SimpleAgentFactory::new();

        let registry = PipelineRegistry::new();
        let pipeline = Pipeline::sequential("empty", vec![]);
        let id = pipeline.id().clone();
        registry.register(pipeline);

        let executor = build_executor(factory, registry);
        let token = CancellationToken::new();

        let output = executor.run(&id, "input".into(), token).await.unwrap();
        assert!(output.steps.is_empty());
        assert!(output.final_response.is_empty());
    }

    #[tokio::test]
    async fn run_unknown_pipeline_returns_not_found() {
        let factory = SimpleAgentFactory::new();
        let registry = PipelineRegistry::new();

        let executor = build_executor(factory, registry);
        let token = CancellationToken::new();
        let unknown_id = PipelineId::new("nonexistent");

        let result = executor.run(&unknown_id, "input".into(), token).await;
        assert!(matches!(
            result,
            Err(PipelineError::PipelineNotFound { id }) if id == unknown_id
        ));
    }
}
