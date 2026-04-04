#![forbid(unsafe_code)]
//! Multi-agent pipeline patterns for swink-agent.
//!
//! This crate provides composable pipeline primitives — sequential, parallel,
//! and loop patterns — for orchestrating multiple agents.

#[cfg(feature = "pipelines")]
pub mod pipeline;

#[cfg(feature = "pipelines")]
pub use pipeline::{
    AgentFactory, ExitCondition, MergeStrategy, Pipeline, PipelineError, PipelineEvent,
    PipelineExecutor, PipelineId, PipelineOutput, PipelineRegistry, SimpleAgentFactory, StepResult,
};
