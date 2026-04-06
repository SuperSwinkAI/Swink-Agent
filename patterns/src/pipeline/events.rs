//! Pipeline lifecycle events.

use std::time::Duration;

use swink_agent::Emission;
use swink_agent::types::Usage;

use super::types::PipelineId;

/// Events emitted during pipeline execution.
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
    pub fn to_emission(&self) -> Emission {
        let kind = match self {
            Self::Started { .. } => "pipeline.started",
            Self::StepStarted { .. } => "pipeline.step_started",
            Self::StepCompleted { .. } => "pipeline.step_completed",
            Self::Completed { .. } => "pipeline.completed",
            Self::Failed { .. } => "pipeline.failed",
        };
        Emission::new(kind, serde_json::Value::Null)
    }
}
