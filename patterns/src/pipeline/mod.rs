//! Pipeline composition patterns for multi-agent orchestration.

mod events;
mod executor;
mod output;
mod registry;
mod types;

pub use events::PipelineEvent;
pub use executor::{AgentFactory, PipelineExecutor, SimpleAgentFactory};
pub use output::{PipelineError, PipelineOutput, StepResult};
pub use registry::PipelineRegistry;
pub use types::{ExitCondition, MergeStrategy, Pipeline, PipelineId};
