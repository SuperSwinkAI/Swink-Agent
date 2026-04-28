//! Multi-turn simulation support for eval case generation and replay.
//!
//! This module implements Phase 6 (US4) of spec 043-evals-adv-features:
//!
//! * [`ActorSimulator`] drives a simulated user across multiple turns.
//! * [`ToolSimulator`] produces schema-valid tool responses, backed by a
//!   [`StateRegistry`] of bounded-history [`StateBucket`]s.
//! * [`run_multiturn_simulation`] orchestrates an agent ↔ actor dialogue up
//!   to `max_turns` or goal-completion.
//!
//! All surfaces gated by the crate-level `simulation` feature. Tests live in
//! `eval/tests/simulation_test.rs`, `eval/tests/simulation_state_test.rs`,
//! and `eval/tests/us4_end_to_end_test.rs`.

#![forbid(unsafe_code)]

pub mod actor;
pub mod orchestrator;
pub mod tool;

pub use actor::{ActorProfile, ActorSimulator, ActorTurn};
pub use orchestrator::{SimulationError, SimulationOutcome, run_multiturn_simulation};
pub use tool::{StateBucket, StateRegistry, ToolCallRecord, ToolSchema, ToolSimulator};
