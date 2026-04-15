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
    ///
    /// Fingerprints are derived from the assistant message's `ToolCall` content
    /// blocks (tool name + arguments), which are stable across invocations.
    /// Falls back to tool result content when no matching tool call is found.
    fn extract_fingerprint(turn: &TurnPolicyContext<'_>) -> Vec<(String, serde_json::Value)> {
        // Build a lookup from tool_call_id -> (name, arguments) from assistant message
        let tool_calls: std::collections::HashMap<&str, (&str, &serde_json::Value)> = turn
            .assistant_message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } => Some((id.as_str(), (name.as_str(), arguments))),
                _ => None,
            })
            .collect();

        turn.tool_results
            .iter()
            .map(|tr| {
                if let Some((name, args)) = tool_calls.get(tr.tool_call_id.as_str()) {
                    // Stable fingerprint: tool name + arguments
                    ((*name).to_string(), (*args).clone())
                } else {
                    // Fallback: use tool result content (still more stable than tool_call_id)
                    ("_unknown".to_string(), serde_json::json!(tr.content))
                }
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

    fn msg_with_tool_calls(calls: &[(&str, &str, serde_json::Value)]) -> AssistantMessage {
        AssistantMessage {
            content: calls
                .iter()
                .map(|(id, name, args)| ContentBlock::ToolCall {
                    id: id.to_string(),
                    name: name.to_string(),
                    arguments: args.clone(),
                    partial_json: None,
                })
                .collect(),
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

        let msg1 = msg_with_tool_calls(&[("id1", "bash", serde_json::json!({"cmd": "ls"}))]);
        let results1 = vec![tool_result("id1", "output1")];
        let turn1 = make_turn_ctx(&msg1, &results1);
        assert!(matches!(
            policy.evaluate(&ctx, &turn1),
            PolicyVerdict::Continue
        ));

        let msg2 = msg_with_tool_calls(&[("id2", "bash", serde_json::json!({"cmd": "pwd"}))]);
        let results2 = vec![tool_result("id2", "output2")];
        let turn2 = make_turn_ctx(&msg2, &results2);
        assert!(matches!(
            policy.evaluate(&ctx, &turn2),
            PolicyVerdict::Continue
        ));

        let msg3 = msg_with_tool_calls(&[("id3", "bash", serde_json::json!({"cmd": "whoami"}))]);
        let results3 = vec![tool_result("id3", "output3")];
        let turn3 = make_turn_ctx(&msg3, &results3);
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

        // Same tool name + args each turn (but tool_call_id could differ)
        let msg = msg_with_tool_calls(&[("id1", "bash", serde_json::json!({"cmd": "ls"}))]);
        let results = vec![tool_result("id1", "same_output")];

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

        let msg = msg_with_tool_calls(&[("id1", "bash", serde_json::json!({"cmd": "ls"}))]);
        let results = vec![tool_result("id1", "same")];

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

        let msg1 = msg_with_tool_calls(&[("id1", "bash", serde_json::json!({"cmd": "ls"}))]);
        let results1 = vec![tool_result("id1", "output_a")];
        let turn1 = make_turn_ctx(&msg1, &results1);
        let _ = policy.evaluate(&ctx, &turn1);

        let msg2 = msg_with_tool_calls(&[("id2", "bash", serde_json::json!({"cmd": "pwd"}))]);
        let results2 = vec![tool_result("id2", "output_b")];
        let turn2 = make_turn_ctx(&msg2, &results2);
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

        let same_args = serde_json::json!({"cmd": "ls"});
        let diff_args = serde_json::json!({"cmd": "pwd"});

        // Push 2 identical, then 1 different, then 2 identical again
        // Should not trigger because the different one breaks the streak
        let same_msg = msg_with_tool_calls(&[("id1", "bash", same_args.clone())]);
        let same_res = vec![tool_result("id1", "same")];
        let diff_msg = msg_with_tool_calls(&[("id2", "bash", diff_args)]);
        let diff_res = vec![tool_result("id2", "different")];

        let t = make_turn_ctx(&same_msg, &same_res);
        let _ = policy.evaluate(&ctx, &t);
        let t = make_turn_ctx(&same_msg, &same_res);
        let _ = policy.evaluate(&ctx, &t);
        let t = make_turn_ctx(&diff_msg, &diff_res);
        let _ = policy.evaluate(&ctx, &t);
        let t = make_turn_ctx(&same_msg, &same_res);
        let _ = policy.evaluate(&ctx, &t);
        let t = make_turn_ctx(&same_msg, &same_res);
        let r = policy.evaluate(&ctx, &t);
        // Last 3: different, same, same — not all identical
        assert!(matches!(r, PolicyVerdict::Continue));
    }

    /// Regression test for #276: identical tool calls with different tool_call_ids
    /// must still be detected as a loop.
    #[test]
    fn fresh_tool_call_ids_still_detected_as_loop() {
        let policy = LoopDetectionPolicy::new(3);
        let usage = Usage::default();
        let cost = Cost::default();
        let state = swink_agent::SessionState::new();
        let ctx = make_ctx(&usage, &cost, &state);

        let args = serde_json::json!({"command": "ls -la"});

        // Each turn has the same tool name + args but a different tool_call_id
        let msg1 = msg_with_tool_calls(&[("call_abc", "bash", args.clone())]);
        let res1 = vec![tool_result("call_abc", "file1.txt")];
        let t1 = make_turn_ctx(&msg1, &res1);
        assert!(matches!(
            policy.evaluate(&ctx, &t1),
            PolicyVerdict::Continue
        ));

        let msg2 = msg_with_tool_calls(&[("call_def", "bash", args.clone())]);
        let res2 = vec![tool_result("call_def", "file1.txt")];
        let t2 = make_turn_ctx(&msg2, &res2);
        assert!(matches!(
            policy.evaluate(&ctx, &t2),
            PolicyVerdict::Continue
        ));

        let msg3 = msg_with_tool_calls(&[("call_ghi", "bash", args.clone())]);
        let res3 = vec![tool_result("call_ghi", "file1.txt")];
        let t3 = make_turn_ctx(&msg3, &res3);
        // Despite fresh IDs each time, the tool name + args are identical → loop detected
        assert!(matches!(policy.evaluate(&ctx, &t3), PolicyVerdict::Stop(_)));
    }
}
