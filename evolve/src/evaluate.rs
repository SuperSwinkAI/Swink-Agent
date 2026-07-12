use crate::mutate::Candidate;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use swink_agent::{
    AgentTool, AgentToolResult, AuthConfig, Cost, ResolvedCredential, SessionState, ToolFuture,
    ToolMetadata,
};
use swink_agent_eval::EvalCaseResult;
use swink_agent_eval::{AgentFactory, EvalCase, EvalError};
use tokio_util::sync::CancellationToken;

/// Evaluation result for a single candidate mutation.
#[derive(Debug, Clone)]
pub struct CandidateResult {
    pub candidate: Candidate,
    pub results: Vec<EvalCaseResult>,
    pub aggregate_score: f64,
    pub cost: Cost,
}

/// `EvalCase.metadata` key used to carry a pending [`ToolDescriptionOverride`]
/// from [`MutatingAgentFactory`] to a cooperating `AgentFactory`.
///
/// `EvalCase` has no first-class field for tool overrides (only
/// `system_prompt`, which every factory already consumes), so this uses the
/// case's documented "arbitrary metadata for user-defined extensions"
/// escape hatch instead. See [`ToolDescriptionOverride::from_case`] and
/// [`apply_tool_description_override`].
pub const TOOL_DESCRIPTION_OVERRIDE_KEY: &str = "__evolve_tool_description_override";

/// A pending override of one tool's description, threaded through
/// `EvalCase.metadata` for `ToolDescription` mutation candidates.
///
/// Factories that build their tool list from scratch per case (as opposed to
/// reusing a fixed list) should call [`Self::from_case`] and, when present,
/// wrap the matching tool with [`apply_tool_description_override`] before
/// constructing the `Agent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDescriptionOverride {
    pub tool_name: String,
    pub description: String,
}

impl ToolDescriptionOverride {
    /// Read a pending override from `case.metadata`, if present.
    #[must_use]
    pub fn from_case(case: &EvalCase) -> Option<Self> {
        case.metadata
            .get(TOOL_DESCRIPTION_OVERRIDE_KEY)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    /// Write this override into `metadata`, merging with any existing keys.
    fn write_into(&self, metadata: &mut serde_json::Value) {
        if !metadata.is_object() {
            *metadata = serde_json::json!({});
        }
        // `write_into` only runs after the `is_object` check above, or on an
        // already-object value, so `as_object_mut` always succeeds.
        if let Some(obj) = metadata.as_object_mut() {
            obj.insert(
                TOOL_DESCRIPTION_OVERRIDE_KEY.to_string(),
                serde_json::to_value(self).expect("ToolDescriptionOverride is always serializable"),
            );
        }
    }
}

/// Wraps an [`AgentTool`] and overrides only its `description()`.
///
/// Every other behavior (execution, schema, approval, auth) delegates
/// unchanged to `inner`. Used to apply a `ToolDescription` mutation candidate
/// without needing to reconstruct the wrapped tool.
pub struct DescriptionOverrideTool {
    inner: Arc<dyn AgentTool>,
    description: String,
}

impl DescriptionOverrideTool {
    #[must_use]
    pub fn new(inner: Arc<dyn AgentTool>, description: impl Into<String>) -> Self {
        Self {
            inner,
            description: description.into(),
        }
    }
}

impl AgentTool for DescriptionOverrideTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn label(&self) -> &str {
        self.inner.label()
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> &serde_json::Value {
        self.inner.parameters_schema()
    }

    fn requires_approval(&self) -> bool {
        self.inner.requires_approval()
    }

    fn metadata(&self) -> Option<ToolMetadata> {
        self.inner.metadata()
    }

    fn execution_root(&self) -> Option<&Path> {
        self.inner.execution_root()
    }

    fn approval_context(&self, params: &serde_json::Value) -> Option<serde_json::Value> {
        self.inner.approval_context(params)
    }

    fn auth_config(&self) -> Option<AuthConfig> {
        self.inner.auth_config()
    }

    fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        cancellation_token: CancellationToken,
        on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        state: Arc<std::sync::RwLock<SessionState>>,
        credential: Option<ResolvedCredential>,
    ) -> ToolFuture<'_> {
        self.inner.execute(
            tool_call_id,
            params,
            cancellation_token,
            on_update,
            state,
            credential,
        )
    }
}

/// Apply a tool-description override to a tool list, wrapping the matching
/// tool in a [`DescriptionOverrideTool`]. Tools that don't match `tool_name`
/// are returned unchanged.
///
/// Logs a warning when `tool_name` matches nothing in `tools` — this is the
/// same silent-no-op failure mode the `ToolDescription` mutation pathway was
/// fixed for, so a mismatch here (e.g. a stale tool name after a rename)
/// should be visible rather than swallowed.
#[must_use]
pub fn apply_tool_description_override(
    tools: Vec<Arc<dyn AgentTool>>,
    override_: &ToolDescriptionOverride,
) -> Vec<Arc<dyn AgentTool>> {
    let mut matched = false;
    let result = tools
        .into_iter()
        .map(|t| {
            if t.name() == override_.tool_name {
                matched = true;
                Arc::new(DescriptionOverrideTool::new(
                    t,
                    override_.description.clone(),
                )) as Arc<dyn AgentTool>
            } else {
                t
            }
        })
        .collect();
    if !matched {
        tracing::warn!(
            tool_name = %override_.tool_name,
            "ToolDescriptionOverride matched no tool in the provided list; candidate will \
             evaluate as a no-op identical to baseline"
        );
    }
    result
}

/// Wraps an inner `AgentFactory`, intercepting `create_agent` to inject a
/// mutated system prompt or tool description for candidate evaluation.
///
/// For `FullPrompt` / `PromptSection` candidates, the modified system prompt
/// is stored on construction; for each eval case, we clone the case and
/// replace `case.system_prompt` before delegating to the inner factory.
///
/// For `ToolDescription` candidates, there is no equivalent first-class
/// `EvalCase` field, so the override is threaded through
/// `case.metadata[TOOL_DESCRIPTION_OVERRIDE_KEY]`
/// ([`ToolDescriptionOverride::from_case`]). A cooperating inner factory reads
/// it back and applies [`apply_tool_description_override`] to its tool list
/// before constructing the `Agent`; a factory that ignores it simply
/// evaluates the candidate as a no-op (identical to baseline), same as any
/// other unmet contract on `EvalCase`.
pub struct MutatingAgentFactory {
    inner: Arc<dyn AgentFactory>,
    override_prompt: Option<String>,
    tool_override: Option<ToolDescriptionOverride>,
}

impl MutatingAgentFactory {
    pub fn new(inner: Arc<dyn AgentFactory>, override_prompt: Option<String>) -> Self {
        Self {
            inner,
            override_prompt,
            tool_override: None,
        }
    }

    /// Also thread a tool-description override through `case.metadata`.
    #[must_use]
    pub fn with_tool_override(mut self, tool_override: Option<ToolDescriptionOverride>) -> Self {
        self.tool_override = tool_override;
        self
    }
}

impl AgentFactory for MutatingAgentFactory {
    fn create_agent(
        &self,
        case: &EvalCase,
    ) -> Result<(swink_agent::Agent, CancellationToken), EvalError> {
        if self.override_prompt.is_none() && self.tool_override.is_none() {
            return self.inner.create_agent(case);
        }

        let mut modified = case.clone();
        if let Some(ref prompt) = self.override_prompt {
            modified.system_prompt = prompt.clone();
        }
        if let Some(ref tool_override) = self.tool_override {
            tool_override.write_into(&mut modified.metadata);
        }
        self.inner.create_agent(&modified)
    }
}
