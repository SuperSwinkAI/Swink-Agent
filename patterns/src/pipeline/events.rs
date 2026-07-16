//! Pipeline lifecycle events.

use std::time::Duration;

use swink_agent::Emission;
use swink_agent::Usage;

use super::types::PipelineId;

/// Events emitted during pipeline execution.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum PipelineEvent {
    /// Pipeline execution began.
    Started {
        pipeline_id: PipelineId,
        pipeline_name: String,
    },
    /// A step/branch began execution.
    StepStarted {
        pipeline_id: PipelineId,
        step_index: usize,
        agent_name: String,
    },
    /// A step/branch completed execution.
    StepCompleted {
        pipeline_id: PipelineId,
        step_index: usize,
        agent_name: String,
        duration: Duration,
        usage: Usage,
    },
    /// Pipeline finished successfully.
    Completed {
        pipeline_id: PipelineId,
        total_duration: Duration,
        total_usage: Usage,
    },
    /// Pipeline failed.
    Failed {
        pipeline_id: PipelineId,
        error_message: String,
    },
}

impl PipelineEvent {
    /// Convert this event to an [`Emission`] for integration with `AgentEvent::Custom`.
    ///
    /// The payload carries the fields each variant holds (pipeline ID, step
    /// info, duration, usage, or error message) so consumers subscribing via
    /// `AgentEvent::Custom` don't just see a bare event name.
    pub fn to_emission(&self) -> Emission {
        let (kind, payload) = match self {
            Self::Started {
                pipeline_id,
                pipeline_name,
            } => (
                "pipeline.started",
                serde_json::json!({
                    "pipeline_id": pipeline_id.to_string(),
                    "pipeline_name": pipeline_name,
                }),
            ),
            Self::StepStarted {
                pipeline_id,
                step_index,
                agent_name,
            } => (
                "pipeline.step_started",
                serde_json::json!({
                    "pipeline_id": pipeline_id.to_string(),
                    "step_index": step_index,
                    "agent_name": agent_name,
                }),
            ),
            Self::StepCompleted {
                pipeline_id,
                step_index,
                agent_name,
                duration,
                usage,
            } => (
                "pipeline.step_completed",
                serde_json::json!({
                    "pipeline_id": pipeline_id.to_string(),
                    "step_index": step_index,
                    "agent_name": agent_name,
                    "duration_ms": duration.as_millis() as u64,
                    "usage": usage,
                }),
            ),
            Self::Completed {
                pipeline_id,
                total_duration,
                total_usage,
            } => (
                "pipeline.completed",
                serde_json::json!({
                    "pipeline_id": pipeline_id.to_string(),
                    "total_duration_ms": total_duration.as_millis() as u64,
                    "total_usage": total_usage,
                }),
            ),
            Self::Failed {
                pipeline_id,
                error_message,
            } => (
                "pipeline.failed",
                serde_json::json!({
                    "pipeline_id": pipeline_id.to_string(),
                    "error_message": error_message,
                }),
            ),
        };
        Emission::new(kind, payload)
    }
}
