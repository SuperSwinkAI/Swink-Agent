//! Ready-to-use application-level policies for [`swink_agent`].
//!
//! This crate provides four policies built entirely against `swink-agent`'s public
//! policy trait API. Each policy is feature-gated independently:
//!
//! - **`prompt-guard`**: [`PromptInjectionGuard`] — blocks prompt injection in user messages (`PreTurn`) and tool results (`PostTurn`)
//! - **`pii`**: [`PiiRedactor`] — redacts personally identifiable information from assistant responses
//! - **`content-filter`**: [`ContentFilter`] — keyword/regex blocklist for assistant output
//! - **`audit`**: [`AuditLogger`] — records every turn to a pluggable sink
//!
//! The crate also serves as a reference example for building custom policies.
#![forbid(unsafe_code)]

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
