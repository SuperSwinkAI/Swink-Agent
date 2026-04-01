//! Async variant of context transformation.
//!
//! [`AsyncContextTransformer`] supports async operations like fetching summaries
//! from an LLM or database before compacting context. It complements the
//! synchronous [`ContextTransformer`](crate::ContextTransformer) used in the
//! hot loop.

use std::future::Future;
use std::pin::Pin;

use crate::context_transformer::CompactionReport;
use crate::types::AgentMessage;

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
    ) -> Pin<Box<dyn Future<Output = Option<CompactionReport>> + Send + 'a>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ContentBlock, LlmMessage, UserMessage};

    fn text_message(text: &str) -> AgentMessage {
        AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: text.to_owned(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))
    }

    #[tokio::test]
    async fn async_transformer_struct_impl() {
        struct OverflowTruncator;

        impl AsyncContextTransformer for OverflowTruncator {
            fn transform<'a>(
                &'a self,
                messages: &'a mut Vec<AgentMessage>,
                overflow: bool,
            ) -> Pin<Box<dyn Future<Output = Option<CompactionReport>> + Send + 'a>> {
                Box::pin(async move {
                    if overflow && messages.len() > 2 {
                        let before = messages.len();
                        messages.truncate(2);
                        Some(CompactionReport {
                            dropped_count: before - 2,
                            tokens_before: 0,
                            tokens_after: 0,
                            overflow: true,
                        })
                    } else {
                        None
                    }
                })
            }
        }

        let transformer = OverflowTruncator;

        // No overflow — no change
        let mut messages = vec![text_message("a"), text_message("b"), text_message("c")];
        let report = transformer.transform(&mut messages, false).await;
        assert!(report.is_none());
        assert_eq!(messages.len(), 3);

        // Overflow — truncate
        let report = transformer.transform(&mut messages, true).await;
        assert!(report.is_some());
        let report = report.unwrap();
        assert_eq!(report.dropped_count, 1);
        assert!(report.overflow);
        assert_eq!(messages.len(), 2);
    }

    #[tokio::test]
    async fn async_transformer_trait_object() {
        struct SummaryInjector;

        impl AsyncContextTransformer for SummaryInjector {
            fn transform<'a>(
                &'a self,
                messages: &'a mut Vec<AgentMessage>,
                _overflow: bool,
            ) -> Pin<Box<dyn Future<Output = Option<CompactionReport>> + Send + 'a>> {
                Box::pin(async move {
                    // Simulate injecting a summary at the start
                    messages.insert(0, text_message("[summary of prior context]"));
                    None // not compaction, just injection
                })
            }
        }

        let transformer: Box<dyn AsyncContextTransformer> = Box::new(SummaryInjector);
        let mut messages = vec![text_message("hello")];
        transformer.transform(&mut messages, false).await;
        assert_eq!(messages.len(), 2);
        if let AgentMessage::Llm(LlmMessage::User(u)) = &messages[0] {
            assert_eq!(
                ContentBlock::extract_text(&u.content),
                "[summary of prior context]"
            );
        } else {
            panic!("expected user message");
        }
    }
}
