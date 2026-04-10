//! Shared message conversion utilities for LLM adapters.
//!
//! Re-exports the [`MessageConverter`] trait and [`convert_messages`] generic
//! function from core, plus adapter-specific helpers.

// Re-export core conversion utilities so existing adapter imports continue to work.
pub use swink_agent::{MessageConverter, convert_messages, extract_tool_schemas};
