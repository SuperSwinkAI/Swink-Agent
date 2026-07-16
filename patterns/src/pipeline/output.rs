//! Pipeline output and error types.

use std::time::Duration;

use swink_agent::Usage;

use super::types::PipelineId;

// ─── StepResult ─────────────────────────────────────────────────────────────

/// Per-step execution telemetry.
#[non_exhaustive]
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

impl StepResult {
    /// Create a step result from its parts.
    #[must_use]
    pub fn new(
        agent_name: impl Into<String>,
        response: impl Into<String>,
        duration: Duration,
        usage: Usage,
    ) -> Self {
        Self {
            agent_name: agent_name.into(),
            response: response.into(),
            duration,
            usage,
        }
    }
}

// ─── PipelineOutput ─────────────────────────────────────────────────────────

/// Structured result from pipeline execution.
#[non_exhaustive]
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

impl PipelineOutput {
    /// Create a pipeline output with the identifying fields; step telemetry
    /// and totals start empty and can be filled in with the `with_*` builders.
    #[must_use]
    pub fn new(pipeline_id: PipelineId, final_response: impl Into<String>) -> Self {
        Self {
            pipeline_id,
            final_response: final_response.into(),
            steps: Vec::new(),
            total_duration: Duration::ZERO,
            total_usage: Usage::default(),
        }
    }

    /// Set the per-step telemetry.
    #[must_use]
    pub fn with_steps(mut self, steps: Vec<StepResult>) -> Self {
        self.steps = steps;
        self
    }

    /// Set the wall-clock time for the entire pipeline.
    #[must_use]
    pub fn with_total_duration(mut self, total_duration: Duration) -> Self {
        self.total_duration = total_duration;
        self
    }

    /// Set the aggregated token usage across all steps.
    #[must_use]
    pub fn with_total_usage(mut self, total_usage: Usage) -> Self {
        self.total_usage = total_usage;
        self
    }
}

// ─── PipelineError ──────────────────────────────────────────────────────────

/// Typed error variants for pipeline execution failures.
#[non_exhaustive]
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
