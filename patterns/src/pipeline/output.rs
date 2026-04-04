//! Pipeline output and error types.

use std::time::Duration;

use swink_agent::types::Usage;

use super::types::PipelineId;

// ─── StepResult ─────────────────────────────────────────────────────────────

/// Per-step execution telemetry.
#[derive(Clone, Debug)]
pub struct StepResult {
    /// Which agent ran this step.
    pub agent_name: String,
    /// The agent's text output.
    pub response: String,
    /// Wall-clock time for this step.
    pub duration: Duration,
    /// Token usage for this step.
    pub usage: Usage,
}

// ─── PipelineOutput ─────────────────────────────────────────────────────────

/// Structured result from pipeline execution.
#[derive(Clone, Debug)]
pub struct PipelineOutput {
    /// Which pipeline produced this output.
    pub pipeline_id: PipelineId,
    /// The pipeline's final text output.
    pub final_response: String,
    /// Per-step telemetry.
    pub steps: Vec<StepResult>,
    /// Wall-clock time for the entire pipeline.
    pub total_duration: Duration,
    /// Aggregated token usage across all steps.
    pub total_usage: Usage,
}

// ─── PipelineError ──────────────────────────────────────────────────────────

/// Typed error variants for pipeline execution failures.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// Named agent not found in factory.
    #[error("agent not found: {name}")]
    AgentNotFound { name: String },
    /// Pipeline ID not in registry.
    #[error("pipeline not found: {id}")]
    PipelineNotFound { id: PipelineId },
    /// A step errored during execution.
    #[error("step {step_index} ({agent_name}) failed: {source}")]
    StepFailed {
        step_index: usize,
        agent_name: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// Loop hit safety cap without meeting exit condition.
    #[error("max iterations reached: {iterations}")]
    MaxIterationsReached { iterations: usize },
    /// Cancellation token was triggered.
    #[error("pipeline cancelled")]
    Cancelled,
    /// Regex compilation or other construction failure.
    #[error("invalid exit condition: {message}")]
    InvalidExitCondition { message: String },
}

