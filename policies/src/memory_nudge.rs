//! Memory nudge policy — heuristic detection of save-worthy content in agent turns.
//!
//! [`MemoryNudgePolicy`] implements [`PostTurnPolicy`] and scans the assistant message
//! text for four categories of save-worthy content:
//!
//! - **Correction** — user corrections ("no, actually…", "don't do X", "use Y instead")
//! - **ExplicitSave** — direct save requests ("remember this", "note that", "keep in mind")
//! - **Decision** — decision statements ("we decided to…", "the plan is…")
//! - **Preference** — configuration/preference declarations ("I prefer", "always use")
//!
//! When a match is found above the configured sensitivity threshold, the policy returns
//! [`PolicyVerdict::Inject`] carrying a single [`AgentMessage`] with a
//! `ContentBlock::Extension { type_name: "memory_nudge", data: <JSON MemoryNudge> }`.
//! The caller is responsible for consuming the extension block and persisting it.
//!
//! # Feature gate
//!
//! This module is only compiled when the `memory-nudge` feature is enabled.
//!
//! # Example
//!
//! ```rust,ignore
//! use swink_agent_policies::{MemoryNudgePolicy, NudgeSensitivity};
//! use std::sync::Arc;
//!
//! let policy = Arc::new(
//!     MemoryNudgePolicy::new().with_sensitivity(NudgeSensitivity::High),
//! );
//! // Add to agent's post_turn_policies slot:
//! // options.with_post_turn_policy(policy)
//! ```

#![forbid(unsafe_code)]
use swink_agent::{
    AgentMessage, ContentBlock, LlmMessage, PolicyContext, PolicyVerdict, PostTurnPolicy,
    TurnPolicyContext, UserMessage,
};

// ─── Category ───────────────────────────────────────────────────────────────

/// The category of save-worthy content detected by [`MemoryNudgePolicy`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryNudgeCategory {
    /// A user correction — the user (or assistant relaying a correction) clarifies
    /// that a previous assumption, approach, or fact was wrong.
    ///
    /// Example phrases: "no, actually", "don't do that", "use X instead".
    Correction,

    /// An explicit request to save information for future recall.
    ///
    /// Example phrases: "remember this", "note that", "keep in mind".
    ExplicitSave,

    /// A decision or plan statement about how to proceed.
    ///
    /// Example phrases: "we decided to", "the plan is", "going forward".
    Decision,

    /// A configuration or style preference the user has stated.
    ///
    /// Example phrases: "I prefer", "always use", "my preference is".
    Preference,
}

impl MemoryNudgeCategory {
    /// Return a stable lowercase string name for serialization.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Correction => "correction",
            Self::ExplicitSave => "explicit_save",
            Self::Decision => "decision",
            Self::Preference => "preference",
        }
    }
}

// ─── Nudge payload ──────────────────────────────────────────────────────────

/// A structured payload emitted when save-worthy content is detected in a turn.
///
/// Serialized as JSON inside a `ContentBlock::Extension { type_name: "memory_nudge" }`.
/// Callers should deserialize the `data` field to recover this struct.
#[derive(Debug, Clone)]
pub struct MemoryNudge {
    /// Which category of save-worthy content was detected.
    pub category: MemoryNudgeCategory,
    /// A short summary of the detected content (≤ 200 characters).
    pub summary: String,
    /// Heuristic confidence score in [0.0, 1.0].
    pub confidence: f32,
    /// Zero-based index of the turn in which the nudge was detected.
    pub turn_number: usize,
}

impl MemoryNudge {
    /// Serialize `self` to a [`serde_json::Value`] for embedding in a content block.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "category": self.category.as_str(),
            "summary": self.summary,
            "confidence": self.confidence,
            "turn_number": self.turn_number,
        })
    }
}

// ─── Sensitivity ────────────────────────────────────────────────────────────

/// Controls how aggressively [`MemoryNudgePolicy`] flags borderline content.
///
/// Each level maps to a minimum confidence threshold: matches with a confidence
/// score strictly below the threshold are suppressed (no nudge emitted).
///
/// | Level  | Threshold | Behavior                                        |
/// |--------|-----------|-------------------------------------------------|
/// | Low    | 0.75      | Only high-confidence, unambiguous matches       |
/// | Medium | 0.55      | Balanced — default for most use cases           |
/// | High   | 0.35      | Catch borderline / partial matches too          |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NudgeSensitivity {
    /// Only high-confidence matches (threshold 0.75).
    Low,
    /// Balanced detection — default (threshold 0.55).
    #[default]
    Medium,
    /// Aggressive detection — catches borderline patterns (threshold 0.35).
    High,
}

impl NudgeSensitivity {
    /// Return the minimum confidence score required to emit a nudge.
    pub const fn threshold(self) -> f32 {
        match self {
            Self::Low => 0.75,
            Self::Medium => 0.55,
            Self::High => 0.35,
        }
    }
}

// ─── Policy ─────────────────────────────────────────────────────────────────

/// A `PostTurnPolicy` that detects save-worthy content via heuristic pattern matching.
///
/// When a match is found above the configured [`NudgeSensitivity`] threshold,
/// the policy returns [`PolicyVerdict::Inject`] with an extension content block
/// (`type_name: "memory_nudge"`) containing a serialized [`MemoryNudge`].
///
/// The policy never returns `Stop` — it is purely additive and non-blocking.
/// Callers are responsible for consuming injected extension blocks.
///
/// # Example
///
/// ```rust,ignore
/// let policy = MemoryNudgePolicy::new().with_sensitivity(NudgeSensitivity::High);
/// ```
#[derive(Debug, Clone)]
pub struct MemoryNudgePolicy {
    sensitivity: NudgeSensitivity,
}

impl MemoryNudgePolicy {
    /// Create a `MemoryNudgePolicy` with [`NudgeSensitivity::Medium`] (default).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sensitivity: NudgeSensitivity::Medium,
        }
    }

    /// Set the sensitivity level.
    #[must_use]
    pub const fn with_sensitivity(mut self, sensitivity: NudgeSensitivity) -> Self {
        self.sensitivity = sensitivity;
        self
    }
}

impl Default for MemoryNudgePolicy {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Heuristic detectors ────────────────────────────────────────────────────

/// Phrase patterns for each category.
///
/// Each entry is a `(phrase, base_confidence)` pair. The policy does a
/// case-insensitive substring search; the first hit wins for that category.
const CORRECTION_PATTERNS: &[(&str, f32)] = &[
    ("no, actually", 0.90),
    ("don't do that", 0.85),
    ("don't do this", 0.85),
    ("use x instead", 0.80),
    ("use y instead", 0.80),
    ("instead of that", 0.75),
    ("that's wrong", 0.80),
    ("that is wrong", 0.80),
    ("incorrect, ", 0.78),
    ("not quite right", 0.75),
    ("actually, ", 0.60),
    ("rather than that", 0.65),
    ("instead, use", 0.70),
    ("you should use", 0.65),
    ("please use", 0.60),
];

const EXPLICIT_SAVE_PATTERNS: &[(&str, f32)] = &[
    ("remember this", 0.95),
    ("remember that", 0.90),
    ("note that", 0.85),
    ("keep in mind", 0.85),
    ("make a note", 0.90),
    ("save this", 0.90),
    ("don't forget", 0.80),
    ("important to remember", 0.85),
    ("worth noting", 0.80),
    ("for future reference", 0.85),
    ("bear in mind", 0.80),
    ("take note", 0.85),
];

const DECISION_PATTERNS: &[(&str, f32)] = &[
    ("we decided to", 0.90),
    ("we've decided to", 0.90),
    ("we have decided to", 0.90),
    ("the plan is", 0.85),
    ("the decision is", 0.88),
    ("going forward, ", 0.80),
    ("going forward we", 0.80),
    ("from now on", 0.80),
    ("we will use", 0.70),
    ("we are going to", 0.72),
    ("we agreed to", 0.85),
    ("we've agreed to", 0.85),
    ("the approach is", 0.75),
    ("our decision:", 0.88),
    ("decided on", 0.75),
];

const PREFERENCE_PATTERNS: &[(&str, f32)] = &[
    ("i prefer ", 0.90),
    ("my preference is", 0.92),
    ("always use", 0.80),
    ("i like to use", 0.82),
    ("i want to use", 0.78),
    ("please always", 0.75),
    ("i always use", 0.82),
    ("i'd like to use", 0.80),
    ("i would like to use", 0.80),
    ("my style is", 0.85),
    ("my convention is", 0.85),
    ("i use ", 0.55),
    ("our convention is", 0.82),
    ("our style is", 0.82),
    ("prefer to use", 0.80),
];

/// Detect a correction in `text`. Returns `Some(confidence)` on first match.
fn detect_correction(text: &str) -> Option<f32> {
    detect_any(text, CORRECTION_PATTERNS)
}

/// Detect an explicit save request in `text`. Returns `Some(confidence)` on first match.
fn detect_explicit_save(text: &str) -> Option<f32> {
    detect_any(text, EXPLICIT_SAVE_PATTERNS)
}

/// Detect a decision statement in `text`. Returns `Some(confidence)` on first match.
fn detect_decision(text: &str) -> Option<f32> {
    detect_any(text, DECISION_PATTERNS)
}

/// Detect a preference declaration in `text`. Returns `Some(confidence)` on first match.
fn detect_preference(text: &str) -> Option<f32> {
    detect_any(text, PREFERENCE_PATTERNS)
}

/// Case-insensitive substring search across a slice of `(phrase, confidence)` pairs.
fn detect_any(text: &str, patterns: &[(&str, f32)]) -> Option<f32> {
    let lower = text.to_lowercase();
    for (phrase, confidence) in patterns {
        if lower.contains(*phrase) {
            return Some(*confidence);
        }
    }
    None
}

/// Truncate `text` to at most `max_chars` Unicode scalar values.
fn truncate_summary(text: &str, max_chars: usize) -> String {
    let mut chars = text.char_indices();
    if let Some((idx, _)) = chars.nth(max_chars) {
        format!("{}…", &text[..idx])
    } else {
        text.to_string()
    }
}

/// Build a `MemoryNudge` and wrap it in an `AgentMessage` extension block.
fn nudge_message(
    category: MemoryNudgeCategory,
    summary: &str,
    confidence: f32,
    turn_number: usize,
) -> AgentMessage {
    let nudge = MemoryNudge {
        category,
        summary: truncate_summary(summary, 200),
        confidence,
        turn_number,
    };
    let data = nudge.to_json();
    // Embed the nudge as an extension block inside a user message so it can
    // be stored in the message history without being forwarded to the LLM
    // (extension blocks are stripped during message conversion).
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Extension {
            type_name: "memory_nudge".to_string(),
            data,
        }],
        timestamp: 0,
        cache_hint: None,
    }))
}

// ─── PostTurnPolicy impl ─────────────────────────────────────────────────────

impl PostTurnPolicy for MemoryNudgePolicy {
    fn name(&self) -> &str {
        "memory-nudge"
    }

    fn evaluate(&self, ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        let text = ContentBlock::extract_text(&turn.assistant_message.content);
        if text.is_empty() {
            return PolicyVerdict::Continue;
        }

        let threshold = self.sensitivity.threshold();
        let mut messages: Vec<AgentMessage> = Vec::new();

        // Check each category in priority order; emit one nudge per matching category.
        type Detector = fn(&str) -> Option<f32>;
        let detectors: &[(Detector, MemoryNudgeCategory)] = &[
            (detect_correction, MemoryNudgeCategory::Correction),
            (detect_explicit_save, MemoryNudgeCategory::ExplicitSave),
            (detect_decision, MemoryNudgeCategory::Decision),
            (detect_preference, MemoryNudgeCategory::Preference),
        ];

        for (detector, category) in detectors {
            if let Some(confidence) = detector(&text)
                && confidence >= threshold
            {
                messages.push(nudge_message(
                    category.clone(),
                    &text,
                    confidence,
                    ctx.turn_index,
                ));
            }
        }

        if messages.is_empty() {
            PolicyVerdict::Continue
        } else {
            PolicyVerdict::Inject(messages)
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use swink_agent::{
        AssistantMessage, ContentBlock, Cost, LlmMessage, PolicyContext, PolicyVerdict, StopReason,
        ToolResultMessage, TurnPolicyContext, Usage, UserMessage,
    };

    // ── Test helpers ─────────────────────────────────────────────────────

    fn make_assistant(text: &str) -> AssistantMessage {
        AssistantMessage {
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
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

    fn make_turn_ctx<'a>(
        assistant: &'a AssistantMessage,
        tool_results: &'a [ToolResultMessage],
        context_messages: &'a [AgentMessage],
    ) -> TurnPolicyContext<'a> {
        static MODEL: std::sync::LazyLock<swink_agent::ModelSpec> =
            std::sync::LazyLock::new(|| swink_agent::ModelSpec::new("test", "test-model"));
        TurnPolicyContext {
            assistant_message: assistant,
            tool_results,
            stop_reason: StopReason::Stop,
            system_prompt: "",
            model_spec: &MODEL,
            context_messages,
        }
    }

    fn make_policy_ctx<'a>(
        usage: &'a Usage,
        cost: &'a Cost,
        state: &'a swink_agent::SessionState,
    ) -> PolicyContext<'a> {
        PolicyContext {
            turn_index: 3,
            accumulated_usage: usage,
            accumulated_cost: cost,
            message_count: 10,
            overflow_signal: false,
            new_messages: &[],
            state,
        }
    }

    fn evaluate_text(text: &str, sensitivity: NudgeSensitivity) -> PolicyVerdict {
        let policy = MemoryNudgePolicy::new().with_sensitivity(sensitivity);
        let assistant = make_assistant(text);
        let usage = Usage::default();
        let cost = Cost::default();
        let state = swink_agent::SessionState::new();
        let ctx = make_policy_ctx(&usage, &cost, &state);
        let turn = make_turn_ctx(&assistant, &[], &[]);
        PostTurnPolicy::evaluate(&policy, &ctx, &turn)
    }

    fn expect_inject_category(verdict: PolicyVerdict, expected: &str) {
        match verdict {
            PolicyVerdict::Inject(msgs) => {
                assert!(!msgs.is_empty(), "expected at least one injected message");
                let msg = &msgs[0];
                if let AgentMessage::Llm(LlmMessage::User(UserMessage { content, .. })) = msg {
                    if let Some(ContentBlock::Extension { type_name, data }) = content.first() {
                        assert_eq!(type_name, "memory_nudge");
                        let category = data["category"].as_str().expect("category must be string");
                        assert_eq!(
                            category, expected,
                            "expected category {expected}, got {category}"
                        );
                        let confidence = data["confidence"].as_f64().expect("confidence required");
                        assert!(
                            confidence > 0.0 && confidence <= 1.0,
                            "confidence out of range: {confidence}"
                        );
                    } else {
                        panic!("expected Extension content block, got: {content:?}");
                    }
                } else {
                    panic!("expected User message, got: {msg:?}");
                }
            }
            other => panic!("expected Inject verdict, got: {other:?}"),
        }
    }

    // ── T094 unit tests ───────────────────────────────────────────────────

    #[test]
    fn correction_phrase_triggers_correction_nudge() {
        let verdict = evaluate_text(
            "No, actually you should use serde_json for that.",
            NudgeSensitivity::Medium,
        );
        expect_inject_category(verdict, "correction");
    }

    #[test]
    fn dont_do_that_triggers_correction_nudge() {
        let verdict = evaluate_text(
            "Don't do that, use the builder pattern instead.",
            NudgeSensitivity::Medium,
        );
        expect_inject_category(verdict, "correction");
    }

    #[test]
    fn remember_this_triggers_explicit_save() {
        let verdict = evaluate_text(
            "Remember this: always run cargo fmt before committing.",
            NudgeSensitivity::Medium,
        );
        expect_inject_category(verdict, "explicit_save");
    }

    #[test]
    fn note_that_triggers_explicit_save() {
        let verdict = evaluate_text(
            "Note that the database password is rotated monthly.",
            NudgeSensitivity::Medium,
        );
        expect_inject_category(verdict, "explicit_save");
    }

    #[test]
    fn we_decided_triggers_decision() {
        let verdict = evaluate_text(
            "We decided to use Postgres for the primary datastore.",
            NudgeSensitivity::Medium,
        );
        expect_inject_category(verdict, "decision");
    }

    #[test]
    fn the_plan_is_triggers_decision() {
        let verdict = evaluate_text(
            "The plan is to migrate to async/await across the board.",
            NudgeSensitivity::Medium,
        );
        expect_inject_category(verdict, "decision");
    }

    #[test]
    fn i_prefer_triggers_preference() {
        let verdict = evaluate_text(
            "I prefer dark mode for all my editors.",
            NudgeSensitivity::Medium,
        );
        expect_inject_category(verdict, "preference");
    }

    #[test]
    fn always_use_triggers_preference() {
        let verdict = evaluate_text(
            "Always use snake_case for variable names.",
            NudgeSensitivity::Medium,
        );
        expect_inject_category(verdict, "preference");
    }

    #[test]
    fn no_signal_returns_continue() {
        let verdict = evaluate_text(
            "The function takes two arguments and returns a string.",
            NudgeSensitivity::Medium,
        );
        assert!(
            matches!(verdict, PolicyVerdict::Continue),
            "ordinary text should return Continue, got: {verdict:?}"
        );
    }

    #[test]
    fn empty_text_returns_continue() {
        let verdict = evaluate_text("", NudgeSensitivity::High);
        assert!(
            matches!(verdict, PolicyVerdict::Continue),
            "empty text should return Continue"
        );
    }

    #[test]
    fn below_threshold_returns_continue() {
        // "i use " has base confidence 0.55 — below Low threshold of 0.75
        let verdict = evaluate_text(
            "In this project, I use a custom allocator.",
            NudgeSensitivity::Low,
        );
        assert!(
            matches!(verdict, PolicyVerdict::Continue),
            "low-confidence match should be suppressed at Low sensitivity, got: {verdict:?}"
        );
    }

    #[test]
    fn high_sensitivity_triggers_on_borderline() {
        // "i use " has base confidence 0.55 — above High threshold of 0.35
        let verdict = evaluate_text(
            "In this project, I use a custom allocator.",
            NudgeSensitivity::High,
        );
        expect_inject_category(verdict, "preference");
    }

    #[test]
    fn turn_number_stored_in_nudge() {
        let policy = MemoryNudgePolicy::new();
        let assistant = make_assistant("Remember this: always validate input.");
        let usage = Usage::default();
        let cost = Cost::default();
        let state = swink_agent::SessionState::new();
        let ctx = PolicyContext {
            turn_index: 7,
            accumulated_usage: &usage,
            accumulated_cost: &cost,
            message_count: 20,
            overflow_signal: false,
            new_messages: &[],
            state: &state,
        };
        let turn = make_turn_ctx(&assistant, &[], &[]);
        match PostTurnPolicy::evaluate(&policy, &ctx, &turn) {
            PolicyVerdict::Inject(msgs) => {
                let msg = &msgs[0];
                if let AgentMessage::Llm(LlmMessage::User(UserMessage { content, .. })) = msg {
                    if let Some(ContentBlock::Extension { data, .. }) = content.first() {
                        let turn_number =
                            data["turn_number"].as_u64().expect("turn_number required");
                        assert_eq!(turn_number, 7, "turn_number should match ctx.turn_index");
                    }
                }
            }
            other => panic!("expected Inject, got: {other:?}"),
        }
    }

    #[test]
    fn summary_truncated_at_200_chars() {
        let long_text = format!("Remember this: {}", "x".repeat(300));
        let truncated = truncate_summary(&long_text, 200);
        // The unicode scalar count should be at most 201 (200 chars + ellipsis char)
        let char_count = truncated.chars().count();
        assert!(
            char_count <= 201,
            "summary should be truncated to ≤201 chars, got {char_count}"
        );
        assert!(
            truncated.ends_with('…'),
            "truncated summary should end with ellipsis"
        );
    }

    #[test]
    fn nudge_sensitivity_thresholds() {
        assert!((NudgeSensitivity::Low.threshold() - 0.75).abs() < f32::EPSILON);
        assert!((NudgeSensitivity::Medium.threshold() - 0.55).abs() < f32::EPSILON);
        assert!((NudgeSensitivity::High.threshold() - 0.35).abs() < f32::EPSILON);
    }

    #[test]
    fn category_as_str_values() {
        assert_eq!(MemoryNudgeCategory::Correction.as_str(), "correction");
        assert_eq!(MemoryNudgeCategory::ExplicitSave.as_str(), "explicit_save");
        assert_eq!(MemoryNudgeCategory::Decision.as_str(), "decision");
        assert_eq!(MemoryNudgeCategory::Preference.as_str(), "preference");
    }

    #[test]
    fn multiple_categories_emit_multiple_nudges() {
        // A message that triggers both ExplicitSave and Decision
        let verdict = evaluate_text(
            "Remember this: we decided to use axum for the web layer.",
            NudgeSensitivity::Medium,
        );
        match verdict {
            PolicyVerdict::Inject(msgs) => {
                assert!(
                    msgs.len() >= 2,
                    "expected nudges for both ExplicitSave and Decision, got {} messages",
                    msgs.len()
                );
            }
            other => panic!("expected Inject, got: {other:?}"),
        }
    }
}
