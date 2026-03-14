//! Post-turn lifecycle hook for real-time memory persistence, metrics flush,
//! or steering logic between turns.
//!
//! The [`PostTurnHook`] trait is invoked after each completed turn in the agent
//! loop, giving callers a chance to persist state, flush metrics, or influence
//! loop continuation before the next turn begins.

use std::future::Future;
use std::pin::Pin;

use crate::types::{AgentMessage, AssistantMessage, Cost, ToolResultMessage, Usage};

// ─── PostTurnContext ────────────────────────────────────────────────────────

/// Snapshot of state provided to the post-turn hook.
#[derive(Debug)]
pub struct PostTurnContext<'a> {
    /// Zero-based index of the just-completed turn.
    pub turn_index: usize,
    /// The assistant message from the just-completed turn.
    pub assistant_message: &'a AssistantMessage,
    /// Tool results produced during this turn (empty if no tools were called).
    pub tool_results: &'a [ToolResultMessage],
    /// Accumulated token usage across all turns so far.
    pub accumulated_usage: &'a Usage,
    /// Accumulated cost across all turns so far.
    pub accumulated_cost: &'a Cost,
    /// The full conversation history at this point.
    pub messages: &'a [AgentMessage],
}

// ─── PostTurnAction ─────────────────────────────────────────────────────────

/// Action returned by a [`PostTurnHook`] to influence loop behavior.
#[derive(Debug)]
pub enum PostTurnAction {
    /// Continue the loop normally.
    Continue,
    /// Stop the loop with an optional reason string.
    Stop(Option<String>),
    /// Inject messages into the conversation before the next turn.
    InjectMessages(Vec<AgentMessage>),
}

// ─── PostTurnHook Trait ─────────────────────────────────────────────────────

/// Hook invoked after each completed turn in the agent loop.
///
/// Use this for real-time memory persistence, metrics flush, budget checks,
/// or steering logic that needs to run between turns.
///
/// # Execution order
///
/// The hook runs after the `TurnEnd` event is emitted and before the loop
/// decides whether to continue, poll follow-ups, or stop.
pub trait PostTurnHook: Send + Sync {
    /// Called after each completed turn. Returns an action that controls
    /// whether the loop continues, stops, or injects messages.
    fn on_turn_end<'a>(
        &'a self,
        ctx: &'a PostTurnContext<'a>,
    ) -> Pin<Box<dyn Future<Output = PostTurnAction> + Send + 'a>>;
}

// ─── Compile-time Send + Sync assertions ────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<PostTurnAction>();
    assert_send_sync::<PostTurnContext<'_>>();
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AssistantMessage, Cost, StopReason, Usage};

    /// A simple hook that always continues.
    struct AlwaysContinueHook;

    impl PostTurnHook for AlwaysContinueHook {
        fn on_turn_end<'a>(
            &'a self,
            _ctx: &'a PostTurnContext<'a>,
        ) -> Pin<Box<dyn Future<Output = PostTurnAction> + Send + 'a>> {
            Box::pin(async { PostTurnAction::Continue })
        }
    }

    /// A hook that stops after a cost threshold.
    struct CostLimitHook {
        max_cost: f64,
    }

    impl PostTurnHook for CostLimitHook {
        fn on_turn_end<'a>(
            &'a self,
            ctx: &'a PostTurnContext<'a>,
        ) -> Pin<Box<dyn Future<Output = PostTurnAction> + Send + 'a>> {
            Box::pin(async move {
                if ctx.accumulated_cost.total > self.max_cost {
                    PostTurnAction::Stop(Some(format!(
                        "cost limit exceeded: {:.4} > {:.4}",
                        ctx.accumulated_cost.total, self.max_cost
                    )))
                } else {
                    PostTurnAction::Continue
                }
            })
        }
    }

    /// A hook that injects a user message after the first turn.
    struct InjectAfterFirstTurnHook;

    impl PostTurnHook for InjectAfterFirstTurnHook {
        fn on_turn_end<'a>(
            &'a self,
            ctx: &'a PostTurnContext<'a>,
        ) -> Pin<Box<dyn Future<Output = PostTurnAction> + Send + 'a>> {
            Box::pin(async move {
                if ctx.turn_index == 0 {
                    PostTurnAction::InjectMessages(vec![AgentMessage::Llm(
                        crate::types::LlmMessage::User(crate::types::UserMessage {
                            content: vec![crate::types::ContentBlock::Text {
                                text: "injected follow-up".into(),
                            }],
                            timestamp: 0,
                        }),
                    )])
                } else {
                    PostTurnAction::Continue
                }
            })
        }
    }

    fn test_message() -> AssistantMessage {
        AssistantMessage {
            content: vec![],
            provider: String::new(),
            model_id: String::new(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        }
    }

    #[tokio::test]
    async fn always_continue_hook_returns_continue() {
        let hook = AlwaysContinueHook;
        let msg = test_message();
        let usage = Usage::default();
        let cost = Cost::default();
        let ctx = PostTurnContext {
            turn_index: 0,
            assistant_message: &msg,
            tool_results: &[],
            accumulated_usage: &usage,
            accumulated_cost: &cost,
            messages: &[],
        };
        let action = hook.on_turn_end(&ctx).await;
        assert!(matches!(action, PostTurnAction::Continue));
    }

    #[tokio::test]
    async fn cost_limit_hook_stops_when_exceeded() {
        let hook = CostLimitHook { max_cost: 1.0 };
        let msg = test_message();
        let usage = Usage::default();
        let cost = Cost {
            total: 1.5,
            ..Cost::default()
        };
        let ctx = PostTurnContext {
            turn_index: 3,
            assistant_message: &msg,
            tool_results: &[],
            accumulated_usage: &usage,
            accumulated_cost: &cost,
            messages: &[],
        };
        let action = hook.on_turn_end(&ctx).await;
        assert!(matches!(action, PostTurnAction::Stop(Some(_))));
    }

    #[tokio::test]
    async fn cost_limit_hook_continues_under_budget() {
        let hook = CostLimitHook { max_cost: 1.0 };
        let msg = test_message();
        let usage = Usage::default();
        let cost = Cost {
            total: 0.5,
            ..Cost::default()
        };
        let ctx = PostTurnContext {
            turn_index: 1,
            assistant_message: &msg,
            tool_results: &[],
            accumulated_usage: &usage,
            accumulated_cost: &cost,
            messages: &[],
        };
        let action = hook.on_turn_end(&ctx).await;
        assert!(matches!(action, PostTurnAction::Continue));
    }

    #[tokio::test]
    async fn inject_hook_injects_on_first_turn() {
        let hook = InjectAfterFirstTurnHook;
        let msg = test_message();
        let usage = Usage::default();
        let cost = Cost::default();
        let ctx = PostTurnContext {
            turn_index: 0,
            assistant_message: &msg,
            tool_results: &[],
            accumulated_usage: &usage,
            accumulated_cost: &cost,
            messages: &[],
        };
        let action = hook.on_turn_end(&ctx).await;
        assert!(matches!(action, PostTurnAction::InjectMessages(ref msgs) if msgs.len() == 1));
    }

    #[tokio::test]
    async fn inject_hook_continues_on_later_turns() {
        let hook = InjectAfterFirstTurnHook;
        let msg = test_message();
        let usage = Usage::default();
        let cost = Cost::default();
        let ctx = PostTurnContext {
            turn_index: 1,
            assistant_message: &msg,
            tool_results: &[],
            accumulated_usage: &usage,
            accumulated_cost: &cost,
            messages: &[],
        };
        let action = hook.on_turn_end(&ctx).await;
        assert!(matches!(action, PostTurnAction::Continue));
    }
}
