//! Loop detection policy — detects repeated tool call patterns.
#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::sync::Mutex;

use swink_agent::{
    AgentMessage, ContentBlock, LlmMessage, PolicyContext, PolicyVerdict, PostTurnPolicy,
    TurnPolicyContext, UserMessage, now_timestamp,
};

/// What to do when a repeated tool call pattern is detected.
#[derive(Debug, Clone)]
pub enum LoopDetectionAction {
    /// Stop the loop entirely.
    Stop,
    /// Inject a steering message to redirect the model.
    Inject(String),
}

/// Detects when the model is stuck in a cycle, calling the same tools with
/// identical arguments repeatedly.
///
/// Uses interior mutability (`Mutex`) to track recent turns. The `lookback`
/// parameter controls how many consecutive identical turns trigger detection.
///
/// # Example
/// ```rust,ignore
/// use swink_agent_policies::LoopDetectionPolicy;
/// use swink_agent::AgentOptions;
///
/// let opts = AgentOptions::new(...)
///     .with_post_turn_policy(
///         LoopDetectionPolicy::new(3)
///             .with_steering("Try a different approach.")
///     );
/// ```
pub struct LoopDetectionPolicy {
    lookback: usize,
    on_detect: LoopDetectionAction,
    history: Mutex<VecDeque<Vec<(String, serde_json::Value)>>>,
}

impl LoopDetectionPolicy {
    /// Create a new `LoopDetectionPolicy`. Default action: `Stop`.
    #[must_use]
    pub const fn new(lookback: usize) -> Self {
        Self {
            lookback,
            on_detect: LoopDetectionAction::Stop,
            history: Mutex::new(VecDeque::new()),
        }
    }

    /// Set the action to inject a steering message instead of stopping.
    #[must_use]
    pub fn with_steering(mut self, message: impl Into<String>) -> Self {
        self.on_detect = LoopDetectionAction::Inject(message.into());
        self
    }

    /// Extract tool call fingerprints from a turn context.
    fn extract_fingerprint(turn: &TurnPolicyContext<'_>) -> Vec<(String, serde_json::Value)> {
        turn.tool_results
            .iter()
            .map(|tr| {
                // Use the tool_call_id prefix (tool name) and content as fingerprint
                // The actual tool name isn't in ToolResultMessage, so we use the content hash
                (tr.tool_call_id.clone(), serde_json::json!(tr.content))
            })
            .collect()
    }

    /// Check if the last `lookback` turns all have the same fingerprint.
    fn is_stuck(&self) -> bool {
        let history = self
            .history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if history.len() < self.lookback {
            return false;
        }

        let recent: Vec<_> = history.iter().rev().take(self.lookback).cloned().collect();
        drop(history);
        if recent.is_empty() {
            return false;
        }

        let first = &recent[0];
        recent.iter().skip(1).all(|turn| turn == first)
    }
}

impl std::fmt::Debug for LoopDetectionPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let history_len = self
            .history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        f.debug_struct("LoopDetectionPolicy")
            .field("lookback", &self.lookback)
            .field("on_detect", &self.on_detect)
            .field("history_len", &history_len)
            .finish()
    }
}

impl PostTurnPolicy for LoopDetectionPolicy {
    fn name(&self) -> &'static str {
        "loop_detection"
    }

    fn evaluate(&self, _ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        let fingerprint = Self::extract_fingerprint(turn);

        let mut history = self
            .history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        history.push_back(fingerprint);
        // Keep only lookback + 1 entries (we need lookback for comparison)
        while history.len() > self.lookback + 1 {
            history.pop_front();
        }
        drop(history);

        if self.is_stuck() {
            match &self.on_detect {
                LoopDetectionAction::Stop => {
                    PolicyVerdict::Stop("loop detected: repeated tool call pattern".to_string())
                }
                LoopDetectionAction::Inject(message) => {
                    let steering_msg = AgentMessage::Llm(LlmMessage::User(UserMessage {
                        content: vec![ContentBlock::Text {
                            text: message.clone(),
                        }],
                        timestamp: now_timestamp(),
                        cache_hint: None,
                    }));
                    PolicyVerdict::Inject(vec![steering_msg])
                }
            }
        } else {
            PolicyVerdict::Continue
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swink_agent::{AssistantMessage, ContentBlock, Cost, StopReason, Usage};

    fn make_ctx<'a>(
        usage: &'a Usage,
        cost: &'a Cost,
        state: &'a swink_agent::SessionState,
    ) -> PolicyContext<'a> {
        PolicyContext {
            turn_index: 0,
            accumulated_usage: usage,
            accumulated_cost: cost,
            message_count: 0,
            overflow_signal: false,
            new_messages: &[],
            state,
        }
    }

    fn make_turn_ctx<'a>(
        msg: &'a AssistantMessage,
        results: &'a [swink_agent::ToolResultMessage],
    ) -> TurnPolicyContext<'a> {
        static MODEL: std::sync::LazyLock<swink_agent::ModelSpec> =
            std::sync::LazyLock::new(|| swink_agent::ModelSpec::new("test", "test-model"));
        TurnPolicyContext {
            assistant_message: msg,
            tool_results: results,
            stop_reason: StopReason::Stop,
            system_prompt: "",
            model_spec: &MODEL,
            context_messages: &[],
        }
    }

    fn dummy_msg() -> AssistantMessage {
        AssistantMessage {
            content: vec![],
            provider: String::new(),
            model_id: String::new(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            error_kind: None,
            timestamp: 0,
            cache_hint: None,
        }
    }

    fn tool_result(id: &str, text: &str) -> swink_agent::ToolResultMessage {
        swink_agent::ToolResultMessage {
            tool_call_id: id.to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            is_error: false,
            timestamp: 0,
            details: serde_json::Value::Null,
            cache_hint: None,
        }
    }

    #[test]
    fn no_repeat_returns_continue() {
        let policy = LoopDetectionPolicy::new(3);
        let usage = Usage::default();
        let cost = Cost::default();
        let state = swink_agent::SessionState::new();
        let ctx = make_ctx(&usage, &cost, &state);
        let msg = dummy_msg();

        let results1 = vec![tool_result("bash_1", "output1")];
        let turn1 = make_turn_ctx(&msg, &results1);
        assert!(matches!(
            policy.evaluate(&ctx, &turn1),
            PolicyVerdict::Continue
        ));

        let results2 = vec![tool_result("bash_2", "output2")];
        let turn2 = make_turn_ctx(&msg, &results2);
        assert!(matches!(
            policy.evaluate(&ctx, &turn2),
            PolicyVerdict::Continue
        ));

        let results3 = vec![tool_result("bash_3", "output3")];
        let turn3 = make_turn_ctx(&msg, &results3);
        assert!(matches!(
            policy.evaluate(&ctx, &turn3),
            PolicyVerdict::Continue
        ));
    }

    #[test]
    fn repeat_within_lookback_returns_stop() {
        let policy = LoopDetectionPolicy::new(3);
        let usage = Usage::default();
        let cost = Cost::default();
        let state = swink_agent::SessionState::new();
        let ctx = make_ctx(&usage, &cost, &state);
        let msg = dummy_msg();

        let results = vec![tool_result("bash_1", "same_output")];

        let turn = make_turn_ctx(&msg, &results);
        let r1 = policy.evaluate(&ctx, &turn);
        assert!(matches!(r1, PolicyVerdict::Continue)); // 1 entry, need 3

        let turn = make_turn_ctx(&msg, &results);
        let r2 = policy.evaluate(&ctx, &turn);
        assert!(matches!(r2, PolicyVerdict::Continue)); // 2 entries, need 3

        let turn = make_turn_ctx(&msg, &results);
        let r3 = policy.evaluate(&ctx, &turn);
        assert!(matches!(r3, PolicyVerdict::Stop(_))); // 3 identical entries -> stuck
    }

    #[test]
    fn repeat_with_steering_returns_inject() {
        let policy = LoopDetectionPolicy::new(2).with_steering("Try something different");
        let usage = Usage::default();
        let cost = Cost::default();
        let state = swink_agent::SessionState::new();
        let ctx = make_ctx(&usage, &cost, &state);
        let msg = dummy_msg();

        let results = vec![tool_result("bash_1", "same")];

        let turn = make_turn_ctx(&msg, &results);
        let _ = policy.evaluate(&ctx, &turn); // 1st

        let turn = make_turn_ctx(&msg, &results);
        let r = policy.evaluate(&ctx, &turn); // 2nd identical -> inject
        assert!(matches!(r, PolicyVerdict::Inject(_)));
    }

    #[test]
    fn different_args_not_detected() {
        let policy = LoopDetectionPolicy::new(2);
        let usage = Usage::default();
        let cost = Cost::default();
        let state = swink_agent::SessionState::new();
        let ctx = make_ctx(&usage, &cost, &state);
        let msg = dummy_msg();

        let results1 = vec![tool_result("bash_1", "output_a")];
        let turn1 = make_turn_ctx(&msg, &results1);
        let _ = policy.evaluate(&ctx, &turn1);

        let results2 = vec![tool_result("bash_1", "output_b")]; // different content
        let turn2 = make_turn_ctx(&msg, &results2);
        let r = policy.evaluate(&ctx, &turn2);
        assert!(matches!(r, PolicyVerdict::Continue));
    }

    #[test]
    fn lookback_window_respected() {
        let policy = LoopDetectionPolicy::new(3);
        let usage = Usage::default();
        let cost = Cost::default();
        let state = swink_agent::SessionState::new();
        let ctx = make_ctx(&usage, &cost, &state);
        let msg = dummy_msg();

        // Push 2 identical, then 1 different, then 2 identical again
        // Should not trigger because the different one breaks the streak
        let same = vec![tool_result("bash_1", "same")];
        let different = vec![tool_result("bash_1", "different")];

        let t = make_turn_ctx(&msg, &same);
        let _ = policy.evaluate(&ctx, &t);
        let t = make_turn_ctx(&msg, &same);
        let _ = policy.evaluate(&ctx, &t);
        let t = make_turn_ctx(&msg, &different);
        let _ = policy.evaluate(&ctx, &t);
        let t = make_turn_ctx(&msg, &same);
        let _ = policy.evaluate(&ctx, &t);
        let t = make_turn_ctx(&msg, &same);
        let r = policy.evaluate(&ctx, &t);
        // Last 3: different, same, same — not all identical
        assert!(matches!(r, PolicyVerdict::Continue));
    }
}
