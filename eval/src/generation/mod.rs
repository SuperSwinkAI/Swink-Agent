//! Experiment generation primitives.
//!
//! This module implements Phase 7 (US5) of spec 043-evals-adv-features:
//!
//! * [`TopicPlanner`] produces an even topic-distribution across a request.
//! * [`ExperimentGenerator`] turns a [`GenerationRequest`] into a validated
//!   [`EvalSet`](crate::types::EvalSet), retrying on malformed judge output
//!   up to a bounded cap and omitting unrecoverable slots.
//!
//! Gated by the `generation` feature.

#![forbid(unsafe_code)]

pub mod experiment;
pub mod topic;

pub use experiment::{
    DEFAULT_RETRY_CAP, ExperimentGenerator, GenerationError, GenerationRequest, ToolDef,
};
pub use topic::{TopicPlanner, TopicSlot};
