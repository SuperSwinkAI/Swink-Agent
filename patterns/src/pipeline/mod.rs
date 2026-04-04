//! Pipeline composition patterns for multi-agent orchestration.

mod events;
mod executor;
mod loop_exec;
mod output;
mod parallel;
mod registry;
mod tool;
mod types;

#[cfg(all(test, feature = "testkit"))]
mod event_tests;

pub use events::PipelineEvent;
pub use executor::{AgentFactory, PipelineExecutor, SimpleAgentFactory};
pub use output::{PipelineError, PipelineOutput, StepResult};
pub use registry::PipelineRegistry;
pub use tool::PipelineTool;
pub use types::{ExitCondition, MergeStrategy, Pipeline, PipelineId};
