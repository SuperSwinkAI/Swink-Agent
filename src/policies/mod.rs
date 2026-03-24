//! Built-in policy implementations for the agent loop.
//!
//! All policies are opt-in — none are enabled by default.
//! Add them to the agent via the builder pattern:
//!
//! ```rust,ignore
//! AgentOptions::new(...)
//!     .with_pre_turn_policy(BudgetPolicy::new().max_cost(5.0))
//!     .with_pre_dispatch_policy(ToolDenyListPolicy::new(["bash"]))
//!     .with_post_turn_policy(LoopDetectionPolicy::new(3))
//! ```
#![forbid(unsafe_code)]

pub mod budget;
pub mod checkpoint;
pub mod deny_list;
pub mod loop_detection;
pub mod max_turns;
pub mod sandbox;

pub use budget::BudgetPolicy;
pub use checkpoint::CheckpointPolicy;
pub use deny_list::ToolDenyListPolicy;
pub use loop_detection::{LoopDetectionAction, LoopDetectionPolicy};
pub use max_turns::MaxTurnsPolicy;
pub use sandbox::SandboxPolicy;
