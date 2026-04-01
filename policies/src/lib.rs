//! Policy implementations for [`swink_agent`].
//!
//! This crate provides all policy implementations built against `swink-agent`'s
//! public policy trait API. Each policy is feature-gated independently:
//!
//! ## Core policies
//!
//! - **`budget`**: [`BudgetPolicy`] — stops the loop when cost or token limits are exceeded
//! - **`max-turns`**: [`MaxTurnsPolicy`] — stops the loop after a configured number of turns
//! - **`deny-list`**: [`ToolDenyListPolicy`] — rejects tool calls by name
//! - **`sandbox`**: [`SandboxPolicy`] — restricts file paths to an allowed root directory
//! - **`loop-detection`**: [`LoopDetectionPolicy`] — detects repeated tool call patterns
//! - **`checkpoint`**: [`CheckpointPolicy`] — persists agent state after each turn
//!
//! ## Application policies
//!
//! - **`prompt-guard`**: [`PromptInjectionGuard`] — blocks prompt injection in user messages and tool results
//! - **`pii`**: [`PiiRedactor`] — redacts personally identifiable information from assistant responses
//! - **`content-filter`**: [`ContentFilter`] — keyword/regex blocklist for assistant output
//! - **`audit`**: [`AuditLogger`] — records every turn to a pluggable sink
#![forbid(unsafe_code)]

#[cfg(any(feature = "prompt-guard", feature = "pii", feature = "content-filter"))]
mod patterns;

// ── Core policies ───────────────────────────────────────────────────────────

#[cfg(feature = "budget")]
mod budget;
#[cfg(feature = "budget")]
pub use budget::BudgetPolicy;

#[cfg(feature = "max-turns")]
mod max_turns;
#[cfg(feature = "max-turns")]
pub use max_turns::MaxTurnsPolicy;

#[cfg(feature = "deny-list")]
mod deny_list;
#[cfg(feature = "deny-list")]
pub use deny_list::ToolDenyListPolicy;

#[cfg(feature = "sandbox")]
mod sandbox;
#[cfg(feature = "sandbox")]
pub use sandbox::SandboxPolicy;

#[cfg(feature = "loop-detection")]
mod loop_detection;
#[cfg(feature = "loop-detection")]
pub use loop_detection::{LoopDetectionAction, LoopDetectionPolicy};

#[cfg(feature = "checkpoint")]
mod checkpoint;
#[cfg(feature = "checkpoint")]
pub use checkpoint::CheckpointPolicy;

// ── Application policies ────────────────────────────────────────────────────

#[cfg(feature = "prompt-guard")]
mod prompt_guard;
#[cfg(feature = "prompt-guard")]
pub use prompt_guard::PromptInjectionGuard;

#[cfg(feature = "pii")]
mod pii_redactor;
#[cfg(feature = "pii")]
pub use pii_redactor::{PiiMode, PiiPattern, PiiRedactor};

#[cfg(feature = "content-filter")]
mod content_filter;
#[cfg(feature = "content-filter")]
pub use content_filter::{ContentFilter, ContentFilterError, FilterRule};

#[cfg(feature = "audit")]
mod audit_logger;
#[cfg(feature = "audit")]
pub use audit_logger::{AuditCost, AuditLogger, AuditRecord, AuditSink, AuditUsage, JsonlAuditSink};
