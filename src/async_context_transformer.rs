//! Async variant of context transformation.
//!
//! [`AsyncContextTransformer`] supports async operations like fetching summaries
//! from an LLM or database before compacting context. It complements the
//! synchronous [`ContextTransformer`](crate::ContextTransformer) used in the
//! hot loop.

use std::future::Future;
use std::pin::Pin;

use crate::context::CompactionReport;
use crate::types::AgentMessage;

/// A boxed future returned by an [`AsyncContextTransformer`].
pub type AsyncTransformFuture<'a> =
    Pin<Box<dyn Future<Output = Option<CompactionReport>> + Send + 'a>>;

/// Async context transformer for operations that require I/O (summary fetching,
/// RAG retrieval, database lookups) before transforming the message context.
///
/// Unlike [`ContextTransformer`](crate::ContextTransformer), this trait's
/// `transform` method is async, making it suitable for pre-turn preparation
/// that involves network calls or other async work.
///
/// # Usage Pattern
///
/// The async transformer runs *before* the synchronous `ContextTransformer` in
/// the turn pipeline. It can inject summary messages, fetch relevant context
/// from a vector store, or perform any async preparation.
pub trait AsyncContextTransformer: Send + Sync {
    /// Transform the context messages asynchronously.
    ///
    /// Called before each LLM turn. The `overflow` flag is true when the
    /// previous turn exceeded the context window.
    ///
    /// Returns `Some(CompactionReport)` if messages were modified, `None` otherwise.
    fn transform<'a>(
        &'a self,
        messages: &'a mut Vec<AgentMessage>,
        overflow: bool,
    ) -> AsyncTransformFuture<'a>;
}

#[cfg(test)]
#[path = "async_context_transformer_tests.rs"]
mod tests;
