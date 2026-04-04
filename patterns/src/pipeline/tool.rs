//! Pipeline-as-tool bridge for supervisor agents.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use swink_agent::tool::{AgentTool, AgentToolResult, ToolFuture};
use swink_agent::schema_for;

use super::executor::PipelineExecutor;
use super::types::PipelineId;

// ─── Parameters ─────────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct PipelineToolParams {
    /// Input text to send to the pipeline.
    input: String,
}

// ─── PipelineTool ───────────────────────────────────────────────────────────

/// Wraps a pipeline as an [`AgentTool`] for use by supervisor agents.
///
/// When executed, the tool invokes `PipelineExecutor::run` for the configured
/// pipeline and returns the pipeline's `final_response` as tool output text.
pub struct PipelineTool {
    pipeline_id: PipelineId,
    pipeline_name: String,
    executor: Arc<PipelineExecutor>,
    description: Option<String>,
    schema: Value,
}

impl PipelineTool {
    /// Create a new pipeline tool for the given pipeline.
    pub fn new(
        pipeline_id: PipelineId,
        pipeline_name: String,
        executor: Arc<PipelineExecutor>,
    ) -> Self {
        Self {
            pipeline_id,
            pipeline_name,
            executor,
            description: None,
            schema: schema_for::<PipelineToolParams>(),
        }
    }

    /// Set a custom description for this tool.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

impl AgentTool for PipelineTool {
    fn name(&self) -> &str {
        &self.pipeline_name
    }

    fn label(&self) -> &str {
        &self.pipeline_name
    }

    fn description(&self) -> &str {
        self.description
            .as_deref()
            .unwrap_or("Execute a multi-agent pipeline")
    }

    fn parameters_schema(&self) -> &Value {
        &self.schema
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: Arc<std::sync::RwLock<swink_agent::SessionState>>,
        _credential: Option<swink_agent::credential::ResolvedCredential>,
    ) -> ToolFuture<'_> {
        Box::pin(async move {
            let parsed: PipelineToolParams = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => return AgentToolResult::error(format!("invalid parameters: {e}")),
            };

            match self
                .executor
                .run(&self.pipeline_id, parsed.input, cancellation_token)
                .await
            {
                Ok(output) => AgentToolResult::text(output.final_response),
                Err(e) => AgentToolResult::error(format!("pipeline error: {e}")),
            }
        })
    }
}

#[cfg(all(test, feature = "testkit"))]
mod tests {
    use super::*;
    use std::sync::Arc;

    use swink_agent::testing::{MockStreamFn, default_convert, default_model, text_only_events};
    use swink_agent::{Agent, AgentOptions, AgentTool};

    use crate::pipeline::executor::SimpleAgentFactory;
    use crate::pipeline::registry::PipelineRegistry;
    use crate::pipeline::types::Pipeline;

    /// Extract text from the first Text content block in an AgentToolResult.
    fn result_text(result: &swink_agent::AgentToolResult) -> String {
        result
            .content
            .iter()
            .find_map(|b| match b {
                swink_agent::ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default()
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

    fn make_executor() -> Arc<PipelineExecutor> {
        let factory = Arc::new(SimpleAgentFactory::new());
        let registry = Arc::new(PipelineRegistry::new());
        Arc::new(PipelineExecutor::new(factory, registry))
    }

    fn make_executor_with_pipeline(
        factory: SimpleAgentFactory,
        pipeline: Pipeline,
    ) -> (Arc<PipelineExecutor>, PipelineId) {
        let id = pipeline.id().clone();
        let registry = Arc::new(PipelineRegistry::new());
        registry.register(pipeline);
        let executor = Arc::new(PipelineExecutor::new(Arc::new(factory), registry));
        (executor, id)
    }

    // T048: PipelineTool schema has `input` parameter
    #[test]
    fn schema_has_input_parameter() {
        let executor = make_executor();
        let tool = PipelineTool::new(
            PipelineId::new("test"),
            "test-pipeline".to_owned(),
            executor,
        );

        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");

        let required = schema["required"].as_array().expect("required array");
        assert!(required.contains(&serde_json::json!("input")));

        let props = schema["properties"].as_object().expect("properties");
        assert!(props.contains_key("input"));
    }

    // T046: PipelineTool returns pipeline's final_response as text
    #[tokio::test]
    async fn returns_final_response_as_text() {
        let mut factory = SimpleAgentFactory::new();
        factory.register("agent-a", || make_text_agent("step-one-output"));
        factory.register("agent-b", || make_text_agent("final-output"));

        let pipeline =
            Pipeline::sequential("tool-test", vec!["agent-a".into(), "agent-b".into()]);
        let (executor, id) = make_executor_with_pipeline(factory, pipeline);

        let tool = PipelineTool::new(id, "test-pipeline".to_owned(), executor);

        let state = Arc::new(std::sync::RwLock::new(swink_agent::SessionState::default()));
        let result = tool
            .execute(
                "call-1",
                serde_json::json!({"input": "hello"}),
                CancellationToken::new(),
                None,
                state,
                None,
            )
            .await;

        let text = result_text(&result);
        assert!(!result.is_error, "expected success, got: {text}");
        assert_eq!(text, "final-output");
    }

    // T047: PipelineTool returns error when pipeline fails (agent not found)
    #[tokio::test]
    async fn returns_error_on_pipeline_failure() {
        let factory = SimpleAgentFactory::new(); // no agents registered

        let pipeline = Pipeline::sequential("failing", vec!["ghost".into()]);
        let (executor, id) = make_executor_with_pipeline(factory, pipeline);

        let tool = PipelineTool::new(id, "fail-pipeline".to_owned(), executor);

        let state = Arc::new(std::sync::RwLock::new(swink_agent::SessionState::default()));
        let result = tool
            .execute(
                "call-1",
                serde_json::json!({"input": "hello"}),
                CancellationToken::new(),
                None,
                state,
                None,
            )
            .await;

        assert!(result.is_error);
        assert!(result_text(&result).contains("pipeline error"));
    }

    #[test]
    fn pipeline_tool_name_and_description() {
        let executor = make_executor();
        let tool = PipelineTool::new(
            PipelineId::new("p1"),
            "my-pipeline".to_owned(),
            executor.clone(),
        );
        assert_eq!(tool.name(), "my-pipeline");
        assert_eq!(tool.label(), "my-pipeline");
        assert_eq!(tool.description(), "Execute a multi-agent pipeline");

        let tool_with_desc = PipelineTool::new(
            PipelineId::new("p2"),
            "described".to_owned(),
            executor,
        )
        .with_description("A custom pipeline description");
        assert_eq!(tool_with_desc.description(), "A custom pipeline description");
    }

    #[test]
    fn pipeline_tool_rejects_invalid_params() {
        let executor = make_executor();
        let tool = PipelineTool::new(
            PipelineId::new("p1"),
            "bad-params".to_owned(),
            executor,
        );

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let result = rt.block_on(async {
            let state = Arc::new(std::sync::RwLock::new(swink_agent::SessionState::default()));
            tool.execute(
                "call-1",
                serde_json::json!({"wrong_field": "hello"}),
                CancellationToken::new(),
                None,
                state,
                None,
            )
            .await
        });

        assert!(result.is_error);
        assert!(result_text(&result).contains("invalid parameters"));
    }

    #[tokio::test]
    async fn returns_error_for_unknown_pipeline_id() {
        let executor = make_executor();
        let tool = PipelineTool::new(
            PipelineId::new("nonexistent"),
            "missing".to_owned(),
            executor,
        );

        let state = Arc::new(std::sync::RwLock::new(swink_agent::SessionState::default()));
        let result = tool
            .execute(
                "call-1",
                serde_json::json!({"input": "hello"}),
                CancellationToken::new(),
                None,
                state,
                None,
            )
            .await;

        assert!(result.is_error);
        assert!(result_text(&result).contains("pipeline error"));
    }
}
