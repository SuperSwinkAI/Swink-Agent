//! Loop detection policy — detects repeated tool call patterns.
#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::sync::Mutex;

use swink_agent::{
    AgentMessage, ContentBlock, LlmMessage, PolicyContext, PolicyVerdict, PostTurnPolicy,
    TurnPolicyContext, UserMessage,
};

/// What to do when a repeated tool call pattern is detected.
#[non_exhaustive]
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
                    let steering_msg = AgentMessage::Llm(LlmMessage::User(UserMessage::new(vec![
                        ContentBlock::Text {
                            text: message.clone(),
                        },
                    ])));
                    PolicyVerdict::Inject(vec![steering_msg])
                }
            }
        } else {
            PolicyVerdict::Continue
        }
    }
}

#[cfg(test)]
#[path = "loop_detection_tests.rs"]
mod tests;
