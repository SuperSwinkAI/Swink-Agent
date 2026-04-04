//! Pipeline executor and agent factory traits.

use std::collections::HashMap;
use std::sync::Arc;

use swink_agent::Agent;

use super::events::PipelineEvent;
use super::output::PipelineError;
use super::registry::PipelineRegistry;

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
        let builder = self.builders.get(name).ok_or_else(|| {
            PipelineError::AgentNotFound {
                name: name.to_owned(),
            }
        })?;
        Ok(builder())
    }
}

// ─── PipelineExecutor ───────────────────────────────────────────────────────

/// Orchestrates pipeline execution using an agent factory and registry.
pub struct PipelineExecutor {
    #[allow(dead_code)]
    factory: Arc<dyn AgentFactory>,
    #[allow(dead_code)]
    registry: Arc<PipelineRegistry>,
    #[allow(dead_code)]
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
}

#[cfg(all(test, feature = "testkit"))]
mod tests {
    use super::*;
    use std::sync::Arc;
    use swink_agent::AgentOptions;
    use swink_agent::testing::{MockStreamFn, default_convert, default_model};

    fn make_agent() -> Agent {
        let options = AgentOptions::new(
            "test",
            default_model(),
            Arc::new(MockStreamFn::new(vec![])),
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
}
